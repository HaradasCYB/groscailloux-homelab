//! Adresse du client d'une requête. `X-Forwarded-For` n'est cru que si la connexion TCP vient du **proxy de
//! confiance** (NPM) : jusqu'au 2026-10-07, il était lu pour tout le monde, et une requête directe sur le port
//! 8766 (un processus de l'hôte, un conteneur du réseau Docker) avec l'IP de la maison dans cet en-tête
//! recevait une session admin d'un an.
//!
//! - Pair TCP local (127.0.0.1, ::1) : jamais cru, même listé. Sans `X-Forwarded-For`, c'est la CLI
//!   (`Client::local`), seule admise sur `/admin/*`.
//! - Pair dans `[web] trusted_proxies` (le réseau Docker) **et**, si `trusted_proxy_container` est rempli, à
//!   l'adresse actuelle de ce conteneur. L'IP de NPM change à chaque recréation (`.15`, `.16`, `.19`…) et le
//!   réseau n'a pas d'IPAM dans le compose : on ne peut pas l'épingler sans recréer toute la pile. Elle est donc
//!   relue par `docker inspect`, gardée `TTL`, relue plus tôt (au plus toutes les `RETRY`) quand un pair du réseau
//!   n'y figure pas (NPM recréé).
//! - Compromis : si Docker ne répond pas, repli sur le réseau entier (tous les conteneurs) plutôt que sur
//!   « personne » : l'admin ne perd ni la session automatique de la maison ni la limite par adresse des pages
//!   publiques. Un conteneur qui reprendrait l'ancienne IP de NPM garde la confiance au plus `TTL`.
//! - Pair non cru : l'adresse retenue est celle du pair. Ni IP de la maison, ni CLI : la connexion par jeton
//!   (`/connexion`) marche toujours.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use homelab_core::net::Cidr;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Durée de vie de l'adresse du conteneur proxy.
const TTL: Duration = Duration::from_secs(60);
/// Délai minimal entre deux relectures quand un pair du réseau n'est pas l'adresse connue.
const RETRY: Duration = Duration::from_secs(10);
/// `docker inspect` borné : la requête attend la réponse.
const DOCKER_TIMEOUT: Duration = Duration::from_secs(3);

/// Qui parle, posé par `admin_auth::layer` dans les extensions de **chaque** requête.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    /// Adresse à journaliser et à limiter : dernier saut de `X-Forwarded-For` derrière le proxy, sinon le pair.
    pub ip: String,
    /// Adresse lue dans `X-Forwarded-For` posé par le proxy de confiance : seule source de l'« IP de la maison ».
    pub via_proxy: bool,
    /// Pair local sans `X-Forwarded-For` : la CLI (`homelabctl` appelle `127.0.0.1:8766`).
    pub local: bool,
}

/// Dernier élément de `X-Forwarded-For` : celui que NPM ajoute (`$proxy_add_x_forwarded_for`) ; les
/// précédents viennent du client et se falsifient. Plusieurs en-têtes : le dernier.
pub fn last_hop(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .next_back()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Le pair TCP est-il le proxy de confiance ? `container` : adresses actuelles du conteneur proxy, `None` quand
/// aucun conteneur n'est nommé ou que Docker n'a pas répondu (le réseau seul décide).
pub fn proxy_trusted(peer: IpAddr, nets: &[Cidr], container: Option<&[IpAddr]>) -> bool {
    let peer = peer.to_canonical();
    if peer.is_loopback() || peer.is_unspecified() {
        return false;
    }
    nets.iter().any(|n| n.contains(peer)) && container.is_none_or(|ips| ips.contains(&peer))
}

/// Décision finale, à partir du pair, du dernier saut et de la confiance accordée au pair.
pub fn resolve(peer: IpAddr, hop: Option<String>, trusted: bool) -> Client {
    let peer = peer.to_canonical();
    match hop {
        Some(ip) if trusted => Client {
            ip,
            via_proxy: true,
            local: false,
        },
        // un `X-Forwarded-For` non cru n'est jamais la CLI : quelque chose relaie
        hop => Client {
            ip: peer.to_string(),
            via_proxy: false,
            local: hop.is_none() && peer.is_loopback(),
        },
    }
}

/// Relire l'adresse du conteneur ? `age` : depuis la dernière lecture (`None` : jamais lue), `known` : le pair
/// est l'adresse connue.
fn should_refresh(age: Option<Duration>, known: bool) -> bool {
    match age {
        None => true,
        Some(a) => a >= TTL || (!known && a >= RETRY),
    }
}

/// Sortie de `docker inspect --format '{{range .NetworkSettings.Networks}}…'` : adresses séparées par des blancs.
fn parse_ips(out: &str) -> Vec<IpAddr> {
    out.split_whitespace()
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .map(|ip| ip.to_canonical())
        .collect()
}

#[derive(Default)]
struct Cache {
    /// Adresses du conteneur à la dernière lecture réussie.
    ips: Vec<IpAddr>,
    /// Dernière lecture tentée, réussie ou non.
    at: Option<Instant>,
    /// La dernière lecture a réussi.
    ok: bool,
    /// Échec déjà journalisé (une ligne par panne, pas une toutes les `RETRY`).
    warned: bool,
}

pub struct ProxyTrust {
    nets: Vec<Cidr>,
    container: String,
    cache: Mutex<Cache>,
}

impl ProxyTrust {
    /// Les entrées invalides sont déjà refusées par `Config::validate` ; ignorées ici par sûreté.
    pub fn new(cfg: &homelab_core::config::Web) -> Self {
        Self {
            nets: cfg
                .trusted_proxies
                .iter()
                .filter_map(|n| Cidr::parse(n).ok())
                .collect(),
            container: cfg.trusted_proxy_container.trim().to_string(),
            cache: Mutex::new(Cache::default()),
        }
    }

    /// `peer` : adresse TCP (`ConnectInfo`) ; absente (jamais en service), la requête n'est ni locale ni relayée.
    pub async fn client(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Client {
        let peer = peer.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let hop = last_hop(headers);
        // sans `X-Forwarded-For`, la confiance ne change rien : pas de `docker inspect`
        let trusted = hop.is_some() && self.trusted(peer).await;
        resolve(peer, hop, trusted)
    }

    async fn trusted(&self, peer: IpAddr) -> bool {
        // filtre gratuit d'abord : un pair hors réseau ou local n'appelle jamais Docker
        if !proxy_trusted(peer, &self.nets, None) {
            return false;
        }
        if self.container.is_empty() {
            return true;
        }
        let peer = peer.to_canonical();
        let mut c = self.cache.lock().await;
        let known = c.ok && c.ips.contains(&peer);
        if should_refresh(c.at.map(|t| t.elapsed()), known) {
            self.refresh(&mut c).await;
        }
        let ips = c.ok.then_some(c.ips.as_slice());
        proxy_trusted(peer, &self.nets, ips)
    }

    async fn refresh(&self, c: &mut Cache) {
        let fmt = "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}";
        let out = tokio::time::timeout(
            DOCKER_TIMEOUT,
            homelab_core::docker::docker(&["inspect", "--format", fmt, &self.container]),
        )
        .await;
        c.at = Some(Instant::now());
        let err = match out {
            Ok(Ok(s)) => {
                let ips = parse_ips(&s);
                if !c.ok || ips != c.ips {
                    info!(task = "web", container = %self.container, ips = ?ips, "adresse du proxy de confiance");
                }
                c.ips = ips;
                c.ok = true;
                c.warned = false;
                return;
            }
            Ok(Err(e)) => e.to_string(),
            Err(_) => "délai dépassé".to_string(),
        };
        if !c.warned {
            warn!(task = "web", container = %self.container, error = %err, "docker inspect en échec : tout le réseau [web] trusted_proxies est cru");
            c.warned = true;
        }
        c.ok = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn docker_net() -> Vec<Cidr> {
        vec![Cidr::parse("172.18.0.0/16").unwrap()]
    }

    fn xff(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn last_hop_is_the_one_npm_appends() {
        assert_eq!(
            last_hop(&xff(&["203.0.113.9, 198.51.100.7"])).as_deref(),
            Some("198.51.100.7")
        );
        assert_eq!(
            last_hop(&xff(&["1.1.1.1", "198.51.100.7"])).as_deref(),
            Some("198.51.100.7")
        );
        assert_eq!(last_hop(&xff(&[" , "])), None);
        assert_eq!(last_hop(&HeaderMap::new()), None);
    }

    #[test]
    fn loopback_is_never_a_proxy() {
        let mut nets = docker_net();
        nets.push(Cidr::parse("127.0.0.0/8").unwrap());
        nets.push(Cidr::parse("::1").unwrap());
        assert!(!proxy_trusted(ip("127.0.0.1"), &nets, None));
        assert!(!proxy_trusted(ip("::1"), &nets, None));
        assert!(!proxy_trusted(ip("::ffff:127.0.0.1"), &nets, None));
        assert!(!proxy_trusted(
            ip("0.0.0.0"),
            &[Cidr::parse("0.0.0.0/0").unwrap()],
            None
        ));
    }

    #[test]
    fn only_npm_is_trusted_when_docker_knows_it() {
        let nets = docker_net();
        let npm = [ip("172.18.0.19")];
        assert!(proxy_trusted(ip("172.18.0.19"), &nets, Some(&npm)));
        assert!(proxy_trusted(ip("::ffff:172.18.0.19"), &nets, Some(&npm)));
        // un autre conteneur du réseau (Guacamole, Homarr…)
        assert!(!proxy_trusted(ip("172.18.0.5"), &nets, Some(&npm)));
        // NPM arrêté : personne
        assert!(!proxy_trusted(ip("172.18.0.19"), &nets, Some(&[])));
        // Docker muet : le réseau seul décide (jamais d'enfermement dehors)
        assert!(proxy_trusted(ip("172.18.0.5"), &nets, None));
        // hors réseau : jamais, même avec l'adresse du conteneur
        assert!(!proxy_trusted(
            ip("10.0.0.2"),
            &nets,
            Some(&[ip("10.0.0.2")])
        ));
    }

    #[test]
    fn forged_header_from_a_local_process_is_ignored() {
        // `curl -H 'X-Forwarded-For: <IP de la maison>' http://127.0.0.1:8766/…`
        let c = resolve(ip("127.0.0.1"), Some("203.0.113.9".into()), false);
        assert_eq!(c.ip, "127.0.0.1");
        assert!(!c.via_proxy);
        assert!(!c.local, "relayé : pas la CLI");
        // la CLI : locale, sans en-tête
        let cli = resolve(ip("127.0.0.1"), None, false);
        assert!(cli.local && !cli.via_proxy);
        assert!(resolve(ip("::1"), None, false).local);
    }

    #[test]
    fn npm_hop_is_the_client() {
        let c = resolve(ip("172.18.0.19"), Some("198.51.100.7".into()), true);
        assert_eq!(
            c,
            Client {
                ip: "198.51.100.7".into(),
                via_proxy: true,
                local: false
            }
        );
        // un conteneur quelconque : son adresse, jamais l'en-tête
        let other = resolve(ip("172.18.0.5"), Some("198.51.100.7".into()), false);
        assert_eq!(other.ip, "172.18.0.5");
        assert!(!other.via_proxy && !other.local);
        // NPM sans en-tête (n'arrive pas) : adresse du pair, ni maison ni CLI
        let bare = resolve(ip("172.18.0.19"), None, true);
        assert_eq!(bare.ip, "172.18.0.19");
        assert!(!bare.via_proxy && !bare.local);
        // IPv4 vue en IPv6 : journalisée en IPv4
        assert_eq!(resolve(ip("::ffff:10.1.2.3"), None, false).ip, "10.1.2.3");
    }

    #[test]
    fn container_address_is_reread_on_a_miss_but_not_in_a_loop() {
        assert!(should_refresh(None, false));
        assert!(!should_refresh(Some(Duration::from_secs(5)), true));
        assert!(
            !should_refresh(Some(Duration::from_secs(5)), false),
            "pas plus d'une relecture / 10 s"
        );
        assert!(
            should_refresh(Some(Duration::from_secs(11)), false),
            "NPM recréé"
        );
        assert!(!should_refresh(Some(Duration::from_secs(30)), true));
        assert!(should_refresh(Some(Duration::from_secs(61)), true), "TTL");
    }

    #[test]
    fn docker_output_is_parsed() {
        assert_eq!(parse_ips("172.18.0.19  \n"), vec![ip("172.18.0.19")]);
        assert_eq!(
            parse_ips("172.18.0.19  172.20.0.3 fd00::5 "),
            vec![ip("172.18.0.19"), ip("172.20.0.3"), ip("fd00::5")]
        );
        assert!(parse_ips("").is_empty());
        assert!(parse_ips("<no value>").is_empty());
        // Docker récent écrit « invalid IP » pour une adresse vide
        assert_eq!(
            parse_ips("172.18.0.19 invalid IP "),
            vec![ip("172.18.0.19")]
        );
    }
}
