# homelabd et homelabctl : développer, installer, exploiter

À lire avant de modifier le code Rust (`crates/`), `homelab.toml`, les unités `systemd/`, ou d'installer un nouveau
binaire. Ce que fait chaque tâche : [AUTOMATION.md](../AUTOMATION.md) ; architecture du démon :
[ARCHITECTURE.md, homelabd en bref](../ARCHITECTURE.md#homelabd-en-bref).

## 1. Changer le comportement ou le code

- **Changement de comportement = changement de `homelab.toml`** (seuils, intervalles) avant changement de code ; une
  valeur réglable ne s'écrit que là, la doc cite la clé.
- **`Config` refuse tout champ inconnu (`deny_unknown_fields`)** : dès qu'une clé nouvelle est dans `homelab.toml`,
  l'ancien `homelabctl` échoue et l'ancien homelabd **ne redémarrerait plus** (y compris un redémarrage par le chien de
  garde ou un reboot). **Installer homelabd ET homelabctl dans la foulée** ; ne pas fusionner dans `main` une clé
  nouvelle avant la fenêtre d'installation (`homelab.toml` de `/opt/homelab` est le fichier de production).
- **`${NOM}` dans `homelab.toml`** (depuis le 09/10) : développé depuis `.env` par homelabd, homelabctl (même pour
  `list`) et `tools/lib/hlconf.py` ; `$${` donne `${` littéral. Utilisé pour `SEEDBOX_PUBLIC_URL`, `SEEDBOX_HOME`,
  `SEEDBOX_USER`. Une variable absente fait refuser le démarrage, avec son nom et les clés qui la citent (jamais sa
  valeur). Un binaire d'avant 9c9075f lit ces chaînes telles quelles : installer homelabd ET homelabctl. Le lien du mail
  d'activation vient de `ONBOARD_PUBLIC_URL`. Un script qui lit `homelab.toml` par `tomllib` sans `hlconf` voit `${…}`.
- Les valeurs par défaut du code restent égales à celles du TOML : test `config::toml_matches_defaults` (seuls `paths`,
  `urls`, `seedbox.*` et `tasks.disabled` sont exemptés).
- **Nouvelle tâche** : un module dans `crates/homelab-core/src/tasks/`, `impl Task`, ajout dans `registry()`, section
  `[tasks.<nom>]` dans `config.rs` + `homelab.toml`, dry-run respecté, tests unitaires de la décision, paragraphe dans
  [AUTOMATION.md](../AUTOMATION.md). Au 09/10 : 32 tâches dans `registry()` (dont `tba_bypass`, dans `tasks.disabled`,
  et `vo_native_guard`) + l'observateur `auto_import`.
- **homelabd est cloisonné** (`ProtectSystem=strict`) : tout nouveau dossier écrit par une tâche va dans `ReadWritePaths`
  de `systemd/homelabd.service` (sinon « Read-only file system »).
- **Journaliser une erreur = `error = format!("{e:#}")`**, jamais `%e` (premier niveau seulement ; depuis le 08/10,
  `e8a5d3f`). Une erreur reqwest
  dont l'URL porte un secret (webhook Discord, `apikey=`, `ApiKey=`, lien d'indexeur) passe d'abord par
  `.map_err(reqwest::Error::without_url)`, sinon `{e:#}` l'écrit en clair dans le journal, dans `last_error` et dans les
  alertes. Les clients qui passent leur clé en en-tête gardent l'URL.
- **Clients HTTP** : un GET coupé est rejoué **une** fois (`SendRetry::send_retry`) seulement sur une coupure de transport
  (`is_request`/`is_connect`, jamais un délai dépassé, jamais un POST/PUT/DELETE). **Les GET à effet** (recherche ou
  téléchargement Prowlarr, `release` d'un Arr, `get_bytes` du canari) **restent en `.send()`** : tout nouveau GET à effet
  aussi. Jellyseerr (Node, keep-alive 5 s) a son client avec `pool_idle_timeout` 4 s. qBittorrent 5.2 nomme son cookie
  `QBT_SID_<port>` : accepté comme `SID`.
- Valider : `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`.

## 2. Construire et installer

- **Le binaire installé se construit depuis l'arbre de travail de `/opt/homelab` tel quel**
  (`cargo build --release --target x86_64-unknown-linux-musl -j4`, 2 vCPU laissés à Jellyfin), **jamais depuis un worktree
  ou un arbre indexé** : le 19/09, des installs ainsi construits ont retiré du binaire en service les routes `/premium`
  d'une autre session (404 sur le lien public). Le worktree ne sert qu'à `fmt/clippy/test` de ce qu'on committe.
- **Une cible cargo (`CARGO_TARGET_DIR`) par worktree** : partagée (ex. `target/lot2-shared`), cargo juge les crates « à
  jour » par date avec une empreinte indépendant du chemin et reprend les artefacts d'un autre arbre (tests d'une autre
  branche, symboles absents). Copier une cible existante (`cp -a`) pour garder les dépendances ; si une cible partagée a
  servi, vérifier que **ses propres tests** figurent dans la sortie.
- Installer : `sudo install target/x86_64-unknown-linux-musl/release/homelab{d,ctl} /usr/local/bin/ && sudo systemctl
  restart homelabd` (coupure de quelques secondes, sans effet sur la lecture). Puis : `systemctl status homelabd`,
  `journalctl -u homelabd -n 50` (aucune erreur de configuration), `curl -s 127.0.0.1:8766/health` (version
  `git describe`), `homelabctl check`, `git status --short`, et `curl 127.0.0.1:8766/<route d'une autre session>`.
- `sudo homelabctl install` copie `systemd/*`, active les unités (jamais un gabarit `@`), pose
  `systemd/journald-homelab.conf` (puis `sudo systemctl restart systemd-journald`, ne coupe rien).

## 3. État, CLI et journal

- **`homelabctl` n'écrit jamais le fichier d'état** : il l'ouvre en lecture seule (`TaskContext::new_read_only`).
  `homelabctl run <tâche>` demande le passage au daemon (`POST /admin/run`, jeton d'onboarding en en-tête, 409 si la
  tâche tourne déjà) ; `--dry-run` reste local et n'écrit rien ; `accounts on|off|delete` (`POST /admin/accounts` : les
  droits Jellyseerr à restaurer sont dans l'état), `onboard`, `accounts link`,
  `mail-test`, `chat announce` passent aussi par l'API locale du daemon. Une tâche ne tourne jamais deux fois en même
  temps (verrou dans `scheduler::run_once`).
- **Écriture de l'état** : sérialisé en mémoire puis écrit en **un** appel par un écrivain unique numéroté (jamais un état
  plus ancien après un plus récent ; fichier temporaire, `fsync`, renommage). `state.update` = durable (rend la main une
  fois écrit) ; `state.update_lazy` = simple tenue (début d'un passage, fin d'un passage calme), écrite avec la sauvegarde
  suivante, au plus tard au tour suivant de `StateStore::run_lazy_flusher` (60 s) ou à l'arrêt : **jamais de donnée qui
  compte en lazy**. Le fichier peut retarder d'une minute sur la mémoire ; `/status.html` lit la mémoire.
- Modifier l'état à la main (retirer une entrée « définitive », un enregistrement) : **daemon arrêté**.
- `homelabctl status` : `running? depuis HH:MM` (début récent sans fin enregistrée) ou `interrompu? JJ/MM HH:MM` (début
  plus vieux que `RUN_TIMEOUT`, 600 s) ; ligne ↳ des erreurs récentes ; `errors` est un total depuis l'origine.
- **Passage calme** (réussi, sans action, résumé identique au précédent) : `run_done` en `debug` (`RUST_LOG=debug` pour
  tout voir). Conséquence : **une tâche qui n'a rien à faire n'écrit plus de `run_done` dans le journal** ; pour connaître
  son prochain passage, lire `task_runs.<tâche>.last_start` / `last_end` dans `state/homelabd.json` et `interval_secs` dans
  `homelab.toml` (prochain passage = `last_end` + intervalle). Le canari résume « lecture OK » (détail dans
  `state.canary.last_detail`).
- `state.vo_native` : dernier changement de langue audio par compte (source, issue, avant, après, clients hors liste) ;
  route `POST /admin/vo-native` (jeton d'onboarding), appelée par `homelabctl accounts vo-native|vo-classic`.

## 4. Alertes admin et Discord

- **Tout passe par `alerts::admin`** (mail + Discord admin), qui journalise chaque envoi (« alerte envoyée », objet et
  canaux seulement) et le note dans `state.alerts` (`/status.html`, `homelabctl status`).
- **Règle** : un appelant qui note « déjà signalé » ne le fait que si l'alerte est partie (`alerts::delivered`) ou s'il
  n'existe aucun canal (`alerts::retry_later(sent, configured)`).
- Ce que ça couvre : tâche en échec `[alerts] fail_streak` (6) fois de suite **et** depuis `fail_minutes` (30), puis un
  message à son retour, au plus `fail_alerts_max` (3) par `fail_alerts_window_mins` (10) ; capacité
  (`disk_pressure.alert_pct`, `seedbox_health.quota_alert_pct`, une alerte par franchissement) ; relance ratée de
  `stack_health` ; `cert_watch`, `backup_watch`, `diun_watch` (quotidiennes, aussi à chaque démarrage, jamais deux fois en
  moins de 20 h pour le même défaut) ; unités root en `OnFailure=homelab-alert@%n.service` (homelab-backup,
  homelabd-watchdog, seedbox-mount-watch, jellyfin-transcodes-purge ; un message par unité et par heure). Ces minuteurs
  sont en `LogLevelMax=notice` : seules les lignes préfixées `<5>`/`<4>` par leurs scripts restent au journal.
- **`/health`** = battement de l'ordonnanceur : 503 `scheduler_stale` après 20 min sans tour de boucle ; le chien de garde
  (`homelabd-watchdog.timer`) relance vers 26 min. **Jamais de seuil par tâche** (un passage lent mais légitime ferait
  redémarrer en boucle).
- **Discord** : deux webhooks dans `.env` (`DISCORD_WEBHOOK_MEMBERS`, `DISCORD_WEBHOOK_ADMIN` avec repli sur membres ;
  `DISCORD_ROLE_MEMBERS` facultatif), **jamais dans le dépôt ni les journaux** (`discord::mask`). `homelabctl discord
  apply` configure par API les 4 Arrs (« Discord membres » : Radarr `onDownload`/`onUpgrade`, Sonarr **`onImportComplete`**
  — un message par téléchargement, jamais `onDownload` = un par épisode ; « Discord admin » : santé) et l'agent Discord de
  Jellyseerr (types 2|8|64|128) ; `test` poste un essai ; `remove` retire tout ; sauvegardes JSON dans
  `backups/discord-<date>/` (elles contiennent l'URL : 600). Chemins Arr toujours préfixés `api/v3/` (la base de
  `ArrClient` est la racine). **Un webhook collé dans une conversation est compromis** : le recréer dans Discord
  (Modifier le salon → Intégrations → Webhooks) et relancer `apply`.
- Journal système : `systemd/journald-homelab.conf` (2 Go, rotation quotidienne, 45 j) ; `SplitMode=uid` gardé
  (`journalctl -u homelabd` sans sudo).

## 5. Pages d'administration (`crates/homelabd/src/admin_auth.rs`, `client_addr.rs`)

- `/`, `/onboard`, `/accounts*`, `/recherche*` (jeton d'onboarding) et `/status*` (jeton d'état ou d'onboarding) passent
  par une couche commune. `/connexion` (POST, jeton dans le corps) pose le cookie `gc_admin` (HMAC du jeton, **1 an**,
  `HttpOnly; Secure; SameSite=Lax`, sans état : **renouveler un jeton ferme les sessions**).
- **Jamais de `token=` dans un lien, une redirection ou un mail** (il finissait en clair dans les journaux NPM) ; un vieux
  lien `?token=` ouvre la session puis redirige sans jeton. La couche réinjecte le jeton **en interne** (requête, ou
  `X-Onboard-Token` pour `POST /onboard`) : les pages n'ont pas bougé ; la CLI garde l'en-tête. Le jeton n'est jamais dans le HTML (jeton de formulaire HMAC).
- 10 échecs / 15 min par IP (POST sans session compris), 100 au total (jamais pour un appel local).
- **`X-Forwarded-For`** (dernier saut) n'est cru que d'un pair TCP de `[web] trusted_proxies` (172.18.0.0/16), jamais
  d'une adresse de l'hôte (127.0.0.1, 172.18.0.1). **IP de la maison** (`HOMELABD_ADMIN_TRUSTED_IPS`) : session d'office
  seulement vue par NPM **confirmé par Docker** (`trusted_proxy_container = "npm"`, adresse relue par `docker inspect`
  toutes les 60 s et 10 s après un pair inconnu) ; Docker muet ⇒ formulaire `/connexion`. Conteneur NPM renommé =
  changer la clé.
- Les limites des pages publiques (`/inscription`, `/premium/*`, `/bienvenue/renouveler`) lisent le saut de NPM (avant le
  07/10 : le premier élément de `X-Forwarded-For`, falsifiable ; et `X-Forwarded-For` posé depuis l'hôte donnait une
  session admin d'un an).
- Chemins d'admin (`/`, `/accounts*`, `/recherche*`, `/status*`, `/onboard`, `/connexion`, `/admin*`) : 404 si `Host` n'est
  ni l'hôte de `ONBOARD_PUBLIC_URL` ni une adresse locale (l'hôte premium envoyait tout à homelabd) ; `/admin/*` (CLI) :
  appel local sans `X-Forwarded-For` seulement ; `POST /onboard` fermé sans jeton configuré.
- `/accounts` et `/recherche` gardent **en plus** l'auth HTTP NPM « admin-outils ».
