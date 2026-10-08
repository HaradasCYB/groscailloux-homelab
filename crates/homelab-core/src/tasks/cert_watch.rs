//! Surveillance du certificat TLS et de la chaîne publique (revue Kaizen du 2026-10-07).
//!
//! Un seul certificat Let's Encrypt (joker, défi DNS-01 DuckDNS) sert les 15 hôtes de NPM, Jellyfin compris. NPM le
//! renouvelle seul à partir de J-30 mais ne remonte rien en cas d'échec (jeton DuckDNS changé, panne DuckDNS) : le
//! premier signal aurait été la panne TLS de tous les hôtes. Le canari de lecture passe par `localhost:8096` et ne
//! voit ni NPM ni TLS.
//!
//! Une fois par jour, depuis l'hôte, vers `[tasks.cert_watch] connect` (NPM) avec le nom public de Jellyfin
//! (`JELLYFIN_PUBLIC_URL`, jamais en dur) :
//!   - lecture de la date d'expiration du certificat présenté (connexion sans vérification, pour la lire même
//!     expiré) ;
//!   - `GET <hôte public><health_path>` : la chaîne NPM → Jellyfin répond ;
//!   - même requête avec vérification complète : le certificat est reconnu pour ce nom.
//!
//! Alerte admin (mail + Discord) sous `warn_days` jours, puis chaque jour tant que ça dure ; rien tant que tout va bien.
//!
//! Aucune bibliothèque X.509 : la date vient d'un décodage DER minimal de `notAfter` (`not_after`), testé sur un
//! vrai certificat.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::alerts::{self, Level};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::now;

pub struct CertWatch;

// ---------------------------------------------------------------------------------------------
// Décodage DER de la date d'expiration
// ---------------------------------------------------------------------------------------------

/// Un élément DER : (étiquette, contenu, reste). Étiquettes sur un octet, longueurs jusqu'à 4 octets.
fn tlv(buf: &[u8]) -> Result<(u8, &[u8], &[u8])> {
    let (&tag, rest) = buf.split_first().context("DER tronqué (étiquette)")?;
    if tag & 0x1f == 0x1f {
        bail!("DER : étiquette longue non gérée");
    }
    let (&first, rest) = rest.split_first().context("DER tronqué (longueur)")?;
    let (len, rest) = if first & 0x80 == 0 {
        (first as usize, rest)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || rest.len() < n {
            bail!("DER : longueur invalide");
        }
        let len = rest[..n].iter().fold(0usize, |a, b| (a << 8) | *b as usize);
        (len, &rest[n..])
    };
    if rest.len() < len {
        bail!("DER tronqué (contenu)");
    }
    Ok((tag, &rest[..len], &rest[len..]))
}

/// `UTCTime` (`YYMMDDHHMMSSZ`) ou `GeneralizedTime` (`YYYYMMDDHHMMSSZ`) → secondes Unix (UTC).
fn parse_time(tag: u8, raw: &[u8]) -> Result<i64> {
    let s = std::str::from_utf8(raw).context("date DER non ASCII")?;
    let (digits, fmt) = match tag {
        0x17 => (s, "%y%m%d%H%M%S"),
        0x18 => (s, "%Y%m%d%H%M%S"),
        t => bail!("DER : étiquette de date inattendue {t:#x}"),
    };
    let digits = digits.strip_suffix('Z').context("date DER sans Z (UTC)")?;
    let dt = chrono::NaiveDateTime::parse_from_str(digits, fmt)
        .with_context(|| format!("date DER illisible : {s}"))?;
    Ok(dt.and_utc().timestamp())
}

/// `notAfter` d'un certificat X.509 en DER, en secondes Unix.
/// Certificate ::= SEQUENCE { tbsCertificate SEQUENCE { [0] version?, serial, signature, issuer,
/// validity SEQUENCE { notBefore, notAfter }, … }, … }
pub fn not_after(der: &[u8]) -> Result<i64> {
    let (tag, cert, _) = tlv(der)?;
    if tag != 0x30 {
        bail!("DER : certificat attendu (SEQUENCE), reçu {tag:#x}");
    }
    let (tag, tbs, _) = tlv(cert)?;
    if tag != 0x30 {
        bail!("DER : tbsCertificate attendu, reçu {tag:#x}");
    }
    let mut rest = tbs;
    let (tag, _, after) = tlv(rest)?;
    if tag == 0xa0 {
        rest = after; // version, explicite et facultative
    }
    for _ in 0..3 {
        rest = tlv(rest)?.2; // serial, algorithme de signature, émetteur
    }
    let (tag, validity, _) = tlv(rest)?;
    if tag != 0x30 {
        bail!("DER : validité attendue, reçu {tag:#x}");
    }
    let (_, _, after_not_before) = tlv(validity)?;
    let (tag, raw, _) = tlv(after_not_before)?;
    parse_time(tag, raw)
}

// ---------------------------------------------------------------------------------------------
// Décision (pure)
// ---------------------------------------------------------------------------------------------

/// Tentatives de la sonde et pause entre deux. Le premier passage a lieu à chaque démarrage de homelabd (0 à 30 s
/// après) : un redémarrage de la machine, de NPM ou de Jellyfin rend NPM injoignable ou Jellyfin muet (502/503/504)
/// pendant une à deux minutes, sans que ce soit une panne.
pub const PROBE_ATTEMPTS: u32 = 3;
pub const PROBE_PAUSE_SECS: u64 = 30;

/// Faut-il refaire une tentative ? Oui tant qu'il en reste, quand la connexion a échoué **ou** que le serveur a
/// répondu 5xx (NPM qui n'a plus d'amont répond 502/503/504, ce que le premier essai prenait pour un verdict).
pub fn should_retry(attempt: u32, connect_failed: bool, status: Option<u16>) -> bool {
    attempt + 1 < PROBE_ATTEMPTS && (connect_failed || status.is_some_and(|s| s >= 500))
}

/// Ce que la sonde a vu.
#[derive(Debug, Default, Clone)]
pub struct Observed {
    /// NPM injoignable ou handshake impossible à la dernière des [`PROBE_ATTEMPTS`] tentatives.
    pub connect_error: Option<String>,
    /// Date d'expiration du certificat présenté.
    pub not_after: Option<i64>,
    /// Statut HTTP de `GET <hôte public><health_path>` à la dernière tentative.
    pub health_status: Option<u16>,
    /// La requête avec vérification complète a échoué alors que la première est passée.
    pub strict_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Grave (certificat expiré ou chaîne cassée) ou simple avertissement (expiration proche).
    pub error: bool,
    /// Identité du défaut, sans les chiffres qui bougent d'un jour à l'autre (« expire dans 12 jours » puis 11) :
    /// sert à ne pas répéter l'alerte du même défaut à chaque redémarrage (voir [`defect_key`]).
    pub kind: &'static str,
    pub title: String,
    pub detail: String,
}

/// Jours entiers restants ; négatif si expiré.
pub fn days_left(not_after: i64, ts: i64) -> i64 {
    (not_after - ts).div_euclid(86_400)
}

/// Les points à signaler, du plus grave au moins grave. Vide : tout va bien.
pub fn assess(o: &Observed, ts: i64, warn_days: i64, host: &str) -> Vec<Problem> {
    let mut out = Vec::new();
    if let Some(e) = &o.connect_error {
        out.push(Problem {
            error: true,
            kind: "npm_injoignable",
            title: "NPM injoignable en TLS".into(),
            detail: format!(
                "La connexion TLS vers NPM avec le nom {host} échoue, même après {PROBE_ATTEMPTS} tentatives espacées de {PROBE_PAUSE_SECS} s : {e}. \
                 Tous les hôtes publics sont probablement hors service (`docker compose ps npm`, \
                 `docker compose logs npm`)."
            ),
        });
        return out;
    }
    let mut expired = false;
    if let Some(na) = o.not_after {
        let left = na - ts;
        if left <= 0 {
            expired = true;
            out.push(Problem {
                error: true,
                kind: "expire",
                title: "certificat expiré".into(),
                detail: format!(
                    "Le certificat servi pour {host} a expiré il y a {} jour(s) : navigateurs et applis refusent \
                     tous les hôtes NPM. Voir `npm/data/logs/letsencrypt.log` et le jeton DuckDNS.",
                    days_left(ts, na).max(0)
                ),
            });
        } else if left < warn_days * 86_400 {
            let days = days_left(na, ts);
            out.push(Problem {
                error: days < 7,
                // le passage sous 7 jours est un changement de gravité : défaut « différent », donc alerté aussitôt
                kind: if days < 7 {
                    "expire_bientot_grave"
                } else {
                    "expire_bientot"
                },
                title: format!("certificat : expire dans {days} jour(s)"),
                detail: format!(
                    "Le certificat joker servi pour {host} expire dans {days} jour(s). NPM tente de le renouveler \
                     toutes les heures à partir de J-30 : s'il n'y arrive pas, l'échec n'apparaît que dans \
                     `npm/data/logs/letsencrypt.log` (`docker logs npm`) — jeton DuckDNS changé, panne de DuckDNS ?"
                ),
            });
        }
    }
    match o.health_status {
        Some(200) => {}
        Some(s) => out.push(Problem {
            error: true,
            kind: "chaine_en_defaut",
            title: "chaîne NPM → Jellyfin en défaut".into(),
            detail: format!(
                "GET https://{host}/health répond HTTP {s} par le chemin public : NPM, l'hôte Jellyfin ou \
                 Jellyfin lui-même est en défaut (le canari de lecture, lui, passe par localhost:8096 et ne le voit pas)."
            ),
        }),
        None => {}
    }
    if let (Some(e), false) = (&o.strict_error, expired) {
        out.push(Problem {
            error: true,
            kind: "certificat_non_reconnu",
            title: "certificat non reconnu pour ce nom".into(),
            detail: format!(
                "La connexion est établie mais le certificat servi pour {host} ne passe pas la vérification \
                 (chaîne ou nom) : {e}. Les navigateurs afficheraient une alerte de sécurité."
            ),
        });
    }
    out.sort_by_key(|p| !p.error);
    out
}

/// Identité de l'ensemble des défauts relevés (2026-10-08) : leurs [`Problem::kind`], triés. `alerts::watch` s'en sert
/// pour ne pas réalerter le même défaut à chaque redémarrage de homelabd ; un défaut en plus, en moins ou plus grave
/// change la clé et alerte aussitôt.
pub fn defect_key(problems: &[Problem]) -> String {
    let mut kinds: Vec<&str> = problems.iter().map(|p| p.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds.join(",")
}

/// Message unique pour tous les points relevés : (grave ?, objet, corps).
pub fn compose(problems: &[Problem]) -> Option<(bool, String, String)> {
    let first = problems.first()?;
    let error = problems.iter().any(|p| p.error);
    let subject = if problems.len() == 1 {
        format!("Certificat / chaîne publique : {}", first.title)
    } else {
        format!(
            "Certificat / chaîne publique : {} (+{} autre(s))",
            first.title,
            problems.len() - 1
        )
    };
    let body = problems
        .iter()
        .map(|p| format!("- {} : {}", p.title, p.detail))
        .collect::<Vec<_>>()
        .join("\n\n");
    Some((
        error,
        subject,
        format!("{body}\n\nSonde quotidienne `cert_watch` : elle reprévient chaque jour tant que le défaut dure."),
    ))
}

/// URL de la sonde : celle de Jellyfin vue de l'extérieur, `health_path` en chemin, port de `connect` (443 par défaut,
/// auquel cas `Url` ne l'écrit pas). Refuse tout ce qui n'est pas du HTTPS avec un nom d'hôte.
pub fn health_url(public_url: &str, health_path: &str, port: u16) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(public_url.trim())
        .with_context(|| "JELLYFIN_PUBLIC_URL illisible".to_string())?;
    if url.scheme() != "https" {
        bail!("JELLYFIN_PUBLIC_URL doit être en https pour surveiller le certificat");
    }
    if url.host_str().is_none_or(str::is_empty) {
        bail!("JELLYFIN_PUBLIC_URL sans nom d'hôte");
    }
    url.set_path(health_path);
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_port(Some(port));
    Ok(url)
}

// ---------------------------------------------------------------------------------------------
// Sonde
// ---------------------------------------------------------------------------------------------

struct Fetched {
    status: u16,
    peer_cert: Option<Vec<u8>>,
}

/// `GET url` en forçant le nom de l'hôte vers `addr` (équivalent de `curl --resolve`). `strict = false` : le
/// certificat n'est pas vérifié (il faut pouvoir lire la date d'un certificat expiré), mais il est relevé.
async fn fetch(url: &reqwest::Url, addr: SocketAddr, strict: bool) -> Result<Fetched> {
    let host = url.host_str().context("URL sans hôte")?;
    let client = reqwest::Client::builder()
        .resolve(host, addr)
        .danger_accept_invalid_certs(!strict)
        .tls_info(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent(concat!(
            "homelabd/",
            env!("CARGO_PKG_VERSION"),
            " cert_watch"
        ))
        .build()?;
    let resp = client.get(url.clone()).send().await?;
    let peer_cert = resp
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|t| t.peer_certificate())
        .map(<[u8]>::to_vec);
    Ok(Fetched {
        status: resp.status().as_u16(),
        peer_cert,
    })
}

#[async_trait]
impl Task for CertWatch {
    fn name(&self) -> &'static str {
        "cert_watch"
    }

    fn label(&self) -> &'static str {
        "Certificat TLS"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.cert_watch.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.cert_watch;
        let addr: SocketAddr = cfg
            .connect
            .parse()
            .with_context(|| format!("[tasks.cert_watch] connect invalide : {}", cfg.connect))?;
        let url = health_url(
            &ctx.secrets.jellyfin_public_url,
            &cfg.health_path,
            addr.port(),
        )?;
        let host = url.host_str().unwrap_or_default().to_string();
        let ts = now();

        // seule la dernière tentative compte : un 502 suivi d'un 200 n'est pas un défaut
        let mut obs = Observed::default();
        for attempt in 0..PROBE_ATTEMPTS {
            obs = Observed::default();
            match fetch(&url, addr, false).await {
                Ok(f) => {
                    obs.health_status = Some(f.status);
                    match f.peer_cert.as_deref().map(not_after) {
                        Some(Ok(t)) => obs.not_after = Some(t),
                        Some(Err(e)) => {
                            warn!(
                                task = "cert_watch",
                                error = format!("{e:#}"),
                                "date d'expiration illisible"
                            )
                        }
                        None => warn!(task = "cert_watch", "aucun certificat relevé (tls_info)"),
                    }
                }
                Err(e) => obs.connect_error = Some(format!("{e:#}")),
            }
            if !should_retry(attempt, obs.connect_error.is_some(), obs.health_status) {
                break;
            }
            // un redémarrage de NPM ou de Jellyfin en cours ne doit pas réveiller l'admin
            info!(
                task = "cert_watch",
                attempt = attempt + 1,
                status = obs.health_status,
                error = obs.connect_error.as_deref(),
                "sonde en défaut, nouvel essai"
            );
            tokio::time::sleep(Duration::from_secs(PROBE_PAUSE_SECS)).await;
        }
        if obs.connect_error.is_none() {
            if let Err(e) = fetch(&url, addr, true).await {
                obs.strict_error = Some(format!("{e:#}"));
            }
        }

        let problems = assess(&obs, ts, cfg.warn_days, &host);
        let state = match (obs.not_after, &obs.connect_error) {
            (_, Some(_)) => "NPM injoignable".to_string(),
            (Some(na), None) => format!(
                "certificat valide encore {} j, /health {}",
                days_left(na, ts),
                obs.health_status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "?".into())
            ),
            (None, None) => "date d'expiration inconnue".to_string(),
        };
        match compose(&problems) {
            None => {
                info!(task = "cert_watch", %state, "ok");
                // retour à la normale : une rechute alertera normalement
                alerts::watch_clear(ctx, self.name()).await;
                Ok(Report::new(state, 0))
            }
            Some((error, subject, body)) => {
                warn!(task = "cert_watch", %subject, "défaut relevé");
                let level = if error { Level::Error } else { Level::Warn };
                // le passage a lieu aussi à chaque démarrage de homelabd : le même défaut n'est pas repris avant 20 h
                let sent = alerts::watch(
                    ctx,
                    self.name(),
                    &defect_key(&problems),
                    level,
                    &subject,
                    &body,
                )
                .await;
                let verb = if sent {
                    "signalé(s)"
                } else {
                    "déjà signalé(s)"
                };
                Ok(Report::new(
                    format!("{state} · {} défaut(s) {verb}", problems.len()),
                    if sent { problems.len() as u32 } else { 0 },
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Certificat de test (ECDSA P-256, CN=test.invalid) : notAfter = 11/11/2027 21:07:35 UTC.
    const CERT_HEX: &str = "3082018330820129a003020102021447eba8f1cba53c70c231ed4567115366be45b037300a06082a8648ce3d04030230173115301306035504030c0c746573742e696e76616c6964301e170d3236313030373231303733355a170d3237313131313231303733355a30173115301306035504030c0c746573742e696e76616c69643059301306072a8648ce3d020106082a8648ce3d0301070342000424911dd84b780a2855cca6122dd520a23661d0b2c81e7ddda848f3207aa95874265cd0dce9758e0cb90031b220d951118792b788663d110d48ce54244807cad2a3533051301d0603551d0e0416041409d33a7972cf0129604e5eef3043053659329640301f0603551d2304183016801409d33a7972cf0129604e5eef3043053659329640300f0603551d130101ff040530030101ff300a06082a8648ce3d04030203480030450220366019034cec34f7fcc5a0bb657926a128f9ecb3764a6b115dd0bd64a4378573022100fe8f385c10c15eece5271cedbf61dd97269d4cdd856909864fc6a8c37f0dbb06";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Élément DER (longueurs courtes ou longues).
    fn der(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if content.len() < 128 {
            out.push(content.len() as u8);
        } else {
            out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
        }
        out.extend(content);
        out
    }

    /// Certificat minimal : seules les positions comptent pour `not_after`.
    fn fake_cert(version: bool, not_before: (u8, &str), not_after: (u8, &str)) -> Vec<u8> {
        let mut tbs = Vec::new();
        if version {
            tbs.extend(der(0xa0, &der(0x02, &[2])));
        }
        tbs.extend(der(0x02, &[1, 2, 3])); // serial
        tbs.extend(der(0x30, &der(0x06, &[42]))); // signature
        tbs.extend(der(0x30, &[0x31; 200])); // émetteur : contenu long
        let mut validity = der(not_before.0, not_before.1.as_bytes());
        validity.extend(der(not_after.0, not_after.1.as_bytes()));
        tbs.extend(der(0x30, &validity));
        tbs.extend(der(0x30, b"subject et reste"));
        der(0x30, &der(0x30, &tbs))
    }

    #[test]
    fn reads_not_after_of_a_real_certificate() {
        // `openssl x509 -noout -enddate` : Nov 11 21:07:35 2027 GMT
        assert_eq!(not_after(&hex(CERT_HEX)).unwrap(), 1_825_967_255);
    }

    #[test]
    fn utc_time_and_generalized_time_both_work() {
        let utc = fake_cert(true, (0x17, "261126215435Z"), (0x17, "270226215435Z"));
        assert_eq!(not_after(&utc).unwrap(), 1_803_678_875); // 26/02/2027 21:54:35 UTC
        let gen = fake_cert(true, (0x18, "20261126215435Z"), (0x18, "20610101000000Z"));
        assert_eq!(not_after(&gen).unwrap(), 2_871_763_200); // 01/01/2061
                                                             // sans version explicite (X.509 v1)
        let v1 = fake_cert(false, (0x17, "261126215435Z"), (0x17, "270226215435Z"));
        assert_eq!(not_after(&v1).unwrap(), 1_803_678_875);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(not_after(&[]).is_err());
        assert!(not_after(b"pas un certificat").is_err());
        let real = hex(CERT_HEX);
        for cut in [1, 5, 40, 120, 200, real.len() - 1] {
            assert!(not_after(&real[..cut]).is_err(), "coupé à {cut}");
        }
        // date mal formée
        let bad = fake_cert(true, (0x17, "261126215435Z"), (0x17, "27XX26215435Z"));
        assert!(not_after(&bad).is_err());
        // pas de Z final
        let no_z = fake_cert(true, (0x17, "261126215435Z"), (0x17, "270226215435"));
        assert!(not_after(&no_z).is_err());
    }

    #[test]
    fn a_connection_error_or_a_5xx_is_retried_but_not_forever() {
        // connexion refusée, 500/502/503/504 : on retente tant qu'il reste un essai
        assert!(should_retry(0, true, None));
        for s in [500, 502, 503, 504] {
            assert!(should_retry(0, false, Some(s)), "HTTP {s}");
            assert!(should_retry(1, false, Some(s)), "HTTP {s} au 2e essai");
        }
        // une réponse saine ou une erreur client est un verdict : pas de nouvel essai
        for s in [200, 204, 301, 401, 404] {
            assert!(!should_retry(0, false, Some(s)), "HTTP {s}");
        }
        assert!(!should_retry(0, false, None));
        // le dernier essai conclut, quoi qu'il ait vu
        assert!(!should_retry(PROBE_ATTEMPTS - 1, true, None));
        assert!(!should_retry(PROBE_ATTEMPTS - 1, false, Some(503)));
        assert_eq!(PROBE_ATTEMPTS, 3);
    }

    const DAY: i64 = 86_400;

    fn healthy(not_after: i64) -> Observed {
        Observed {
            not_after: Some(not_after),
            health_status: Some(200),
            ..Default::default()
        }
    }

    #[test]
    fn a_healthy_chain_with_time_left_says_nothing() {
        let now = 1_000_000;
        assert!(assess(&healthy(now + 50 * DAY), now, 21, "h").is_empty());
        // pile 21 jours : pas encore
        assert!(assess(&healthy(now + 21 * DAY), now, 21, "h").is_empty());
        assert_eq!(compose(&[]), None);
    }

    #[test]
    fn under_21_days_it_warns_and_under_7_it_is_serious() {
        let now = 1_000_000;
        let p = assess(&healthy(now + 20 * DAY + 5), now, 21, "jf.example.org");
        assert_eq!(p.len(), 1);
        assert!(!p[0].error, "20 jours : avertissement");
        assert!(p[0].title.contains("20 jour"), "{}", p[0].title);
        assert!(p[0].detail.contains("jf.example.org") && p[0].detail.contains("letsencrypt.log"));
        let p = assess(&healthy(now + 3 * DAY), now, 21, "h");
        assert!(p[0].error && p[0].title.contains("3 jour"));
    }

    #[test]
    fn an_expired_certificate_is_serious_and_hides_the_redundant_strict_error() {
        let now = 10 * DAY;
        let mut o = healthy(now - 2 * DAY - 10);
        o.strict_error = Some("certificate has expired".into());
        let p = assess(&o, now, 21, "h");
        assert_eq!(p.len(), 1, "l'erreur stricte n'ajoute rien à « expiré »");
        assert!(p[0].error && p[0].title == "certificat expiré");
        assert!(p[0].detail.contains("il y a 2 jour"), "{}", p[0].detail);
    }

    #[test]
    fn a_broken_npm_to_jellyfin_chain_is_reported_even_with_a_good_certificate() {
        let now = 1_000_000;
        let mut o = healthy(now + 50 * DAY);
        o.health_status = Some(502);
        let p = assess(&o, now, 21, "h");
        assert_eq!(p.len(), 1);
        assert!(
            p[0].error
                && p[0].detail.contains("HTTP 502")
                && p[0].detail.contains("localhost:8096")
        );
    }

    #[test]
    fn an_unreachable_npm_is_the_only_finding() {
        let o = Observed {
            connect_error: Some("connection refused".into()),
            ..Default::default()
        };
        let p = assess(&o, 0, 21, "h");
        assert_eq!(p.len(), 1);
        assert!(p[0].detail.contains("connection refused"));
    }

    #[test]
    fn a_name_mismatch_is_caught_by_the_strict_request() {
        let now = 1_000_000;
        let mut o = healthy(now + 50 * DAY);
        o.strict_error = Some("invalid peer certificate: NotValidForName".into());
        let p = assess(&o, now, 21, "h");
        assert_eq!(p.len(), 1);
        assert!(p[0].detail.contains("NotValidForName"));
    }

    #[test]
    fn several_findings_make_one_message_with_the_worst_first() {
        let now = 1_000_000;
        let mut o = healthy(now + 10 * DAY); // avertissement
        o.health_status = Some(503); // grave
        let p = assess(&o, now, 21, "h");
        assert_eq!(p.len(), 2);
        assert!(p[0].error, "le plus grave en tête");
        let (error, subject, body) = compose(&p).unwrap();
        assert!(error);
        assert!(
            subject.starts_with("Certificat / chaîne publique : chaîne NPM → Jellyfin")
                && subject.ends_with("(+1 autre(s))"),
            "{subject}"
        );
        assert!(body.contains("expire dans 10 jour"), "{body}");
        assert!(body.contains("chaque jour"), "{body}");
    }

    #[test]
    fn the_defect_key_ignores_the_numbers_that_move_every_day() {
        let now = 1_000_000;
        // « expire dans 12 jours » puis « 11 jours » : même défaut, donc même clé (pas de nouvelle alerte au redémarrage)
        let k12 = defect_key(&assess(&healthy(now + 12 * DAY + 5), now, 21, "h"));
        let k11 = defect_key(&assess(&healthy(now + 11 * DAY + 5), now + DAY, 21, "h"));
        assert_eq!(k12, k11);
        assert_eq!(k12, "expire_bientot");
        // passage sous 7 jours : plus grave, donc défaut différent
        let k5 = defect_key(&assess(&healthy(now + 5 * DAY), now, 21, "h"));
        assert_eq!(k5, "expire_bientot_grave");
        assert_ne!(k12, k5);
        // un défaut de plus change la clé, quel que soit l'ordre dans lequel ils sont relevés
        let mut o = healthy(now + 12 * DAY + 5);
        o.health_status = Some(502);
        let both = defect_key(&assess(&o, now, 21, "h"));
        assert_eq!(both, "chaine_en_defaut,expire_bientot");
        assert_ne!(both, k12);
        // le code HTTP exact n'est pas l'identité du défaut
        o.health_status = Some(503);
        assert_eq!(defect_key(&assess(&o, now, 21, "h")), both);
        // tout va bien : pas de clé
        assert_eq!(
            defect_key(&assess(&healthy(now + 50 * DAY), now, 21, "h")),
            ""
        );
    }

    #[test]
    fn the_probe_url_comes_from_the_public_url_and_the_connect_port() {
        let u = health_url("https://jf.example.org/", "/health", 443).unwrap();
        assert_eq!(u.as_str(), "https://jf.example.org/health");
        let u = health_url("https://jf.example.org/web/?x=1#y", "/health", 8443).unwrap();
        assert_eq!(u.as_str(), "https://jf.example.org:8443/health");
        assert!(
            health_url("http://jf.example.org", "/health", 443).is_err(),
            "pas de TLS à surveiller"
        );
        assert!(health_url("pas une url", "/health", 443).is_err());
        assert!(health_url("https://", "/health", 443).is_err());
    }

    #[test]
    fn days_left_rounds_down_and_goes_negative() {
        assert_eq!(days_left(50 * DAY + 100, 0), 50);
        assert_eq!(days_left(0, 2 * DAY), -2);
    }
}
