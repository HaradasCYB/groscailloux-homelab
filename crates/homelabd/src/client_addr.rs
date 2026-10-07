//! Adresse du client d'une requête. `X-Forwarded-For` n'est cru que si la connexion TCP vient du **proxy de
//! confiance** (NPM) : jusqu'au 2026-10-07, il était lu pour tout le monde, et une requête directe sur le port
//! 8766 (un processus de l'hôte, un conteneur du réseau Docker) avec l'IP de la maison dans cet en-tête
//! recevait une session admin d'un an.
//!
//! - Pair TCP de l'hôte lui-même (127.0.0.1, ::1, mais aussi 172.18.0.1, l'hôte sur le pont Docker) : jamais
//!   cru, même listé. Une adresse est « de l'hôte » quand on peut y lier une socket (`own_address`). Sans
//!   `X-Forwarded-For` et en boucle locale, c'est la CLI (`Client::local`), seule admise sur `/admin/*`.
//! - Pair dans `[web] trusted_proxies` (le réseau Docker) **et**, si `trusted_proxy_container` est rempli, à
//!   l'adresse actuelle de ce conteneur : `Trust::Confirmed`, seule source de l'« IP de la maison ». L'IP de NPM
//!   change à chaque recréation (`.15`, `.16`, `.19`…) et le réseau n'a pas d'IPAM dans le compose : on ne peut
//!   pas l'épingler sans recréer toute la pile. Une tâche de fond la relit par `docker inspect` toutes les `TTL`,
//!   et plus tôt (au plus toutes les `RETRY`) quand un pair du réseau n'y figure pas (NPM recréé). Les requêtes
//!   ne lisent qu'un instantané : elles n'attendent jamais Docker (sauf la toute première lecture, au démarrage).
//! - Docker muet, ou aucun conteneur nommé : `Trust::Network`. L'en-tête sert encore de clé aux limites par
//!   adresse des pages publiques, mais n'ouvre **aucune** session automatique de la maison (2026-10-07 : en repli,
//!   tout conteneur du réseau aurait pu se faire passer pour la maison ; la connexion par jeton marche toujours,
//!   le repli n'enferme donc personne dehors).
//! - Pair non cru : l'adresse retenue est celle du pair. Ni IP de la maison, ni CLI : la connexion par jeton
//!   (`/connexion`) marche toujours.

use std::collections::HashMap;
use std::future::Future;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::http::HeaderMap;
use homelab_core::net::Cidr;
use tokio::sync::{watch, Notify};
// horloge de tokio (et non `std`) : les tests avancent le temps sans attendre
use tokio::time::Instant;
use tracing::{info, warn};

/// Durée de vie de l'adresse du conteneur proxy.
const TTL: Duration = Duration::from_secs(60);
/// Délai minimal entre deux relectures quand un pair du réseau n'est pas l'adresse connue.
const RETRY: Duration = Duration::from_secs(10);
/// `docker inspect` borné : seule la tâche de relecture l'attend (et la toute première requête).
const DOCKER_TIMEOUT: Duration = Duration::from_secs(3);
/// Réponses de `own_address` gardées en mémoire, au plus (seuls des pairs de `trusted_proxies` y entrent).
const OWN_CACHE_MAX: usize = 1024;

/// Qui parle, posé par `admin_auth::layer` dans les extensions de **chaque** requête.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    /// Adresse à journaliser et à limiter : dernier saut de `X-Forwarded-For` derrière le proxy, sinon le pair.
    pub ip: String,
    /// Adresse lue dans `X-Forwarded-For` d'un pair du réseau des proxys (`Trust::Network` ou `Confirmed`).
    pub via_proxy: bool,
    /// Le pair est le conteneur proxy, confirmé par Docker (`Trust::Confirmed`) : seule source de l'« IP de la
    /// maison ». Faux quand Docker ne répond pas ou qu'aucun conteneur n'est nommé.
    pub proxy_confirmed: bool,
    /// Pair local sans `X-Forwarded-For` : la CLI (`homelabctl` appelle `127.0.0.1:8766`).
    pub local: bool,
}

/// Confiance accordée au pair TCP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// `X-Forwarded-For` ignoré : l'adresse retenue est celle du pair.
    Ignored,
    /// Pair du réseau des proxys, sans confirmation de Docker : l'en-tête sert aux limites par adresse, jamais à
    /// l'IP de la maison.
    Network,
    /// Pair à l'adresse actuelle du conteneur proxy, lue par Docker.
    Confirmed,
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

/// Confiance due au pair TCP. `own` : le pair est une adresse de l'hôte (`own_address`) ; `container` :
/// adresses actuelles du conteneur proxy, `None` quand aucun conteneur n'est nommé ou que Docker n'a pas
/// répondu (le réseau seul décide, sans confirmation).
pub fn proxy_trust(peer: IpAddr, nets: &[Cidr], own: bool, container: Option<&[IpAddr]>) -> Trust {
    let peer = peer.to_canonical();
    if own || peer.is_loopback() || peer.is_unspecified() || !nets.iter().any(|n| n.contains(peer))
    {
        return Trust::Ignored;
    }
    match container {
        None => Trust::Network,
        Some(ips) if ips.contains(&peer) => Trust::Confirmed,
        Some(_) => Trust::Ignored,
    }
}

/// Décision finale, à partir du pair, du dernier saut et de la confiance accordée au pair.
pub fn resolve(peer: IpAddr, hop: Option<String>, trust: Trust) -> Client {
    let peer = peer.to_canonical();
    match hop {
        Some(ip) if trust != Trust::Ignored => Client {
            ip,
            via_proxy: true,
            proxy_confirmed: trust == Trust::Confirmed,
            local: false,
        },
        // un `X-Forwarded-For` non cru n'est jamais la CLI : quelque chose relaie
        hop => Client {
            ip: peer.to_string(),
            via_proxy: false,
            proxy_confirmed: false,
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

/// L'adresse est-elle à l'hôte ? On essaie d'y lier une socket UDP (port libre, rien n'est envoyé) : 127.0.0.1
/// et 172.18.0.1 se lient, l'adresse d'un conteneur répond « Cannot assign requested address ». `None` : autre
/// erreur, indéterminé (traité comme à l'hôte, sans être retenu).
fn own_address(ip: IpAddr) -> Option<bool> {
    match UdpSocket::bind(SocketAddr::new(ip, 0)) {
        Ok(_) => Some(true),
        Err(e) if e.kind() == ErrorKind::AddrNotAvailable => Some(false),
        Err(_) => None,
    }
}

type InspectFuture = Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, String>> + Send>>;
/// Lecture des adresses du conteneur proxy : `docker inspect` en service, simulée par les tests.
type Inspect = Arc<dyn Fn() -> InspectFuture + Send + Sync>;
/// Adresse de l'hôte ? (`own_address` en service.)
type OwnAddress = Arc<dyn Fn(IpAddr) -> Option<bool> + Send + Sync>;

fn docker_inspect(container: String) -> Inspect {
    Arc::new(move || {
        let container = container.clone();
        Box::pin(async move {
            let fmt = "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}";
            // `--type container` : jamais un réseau, un volume ou une image du même nom
            homelab_core::docker::docker(&[
                "inspect",
                "--type",
                "container",
                "--format",
                fmt,
                &container,
            ])
            .await
            .map(|s| parse_ips(&s))
            .map_err(|e| e.to_string())
        })
    })
}

/// Dernière lecture du conteneur proxy, la seule chose que lisent les requêtes.
#[derive(Clone, Default)]
struct Snapshot {
    /// Adresses du conteneur à la dernière lecture réussie.
    ips: Vec<IpAddr>,
    /// Dernière lecture tentée, réussie ou non (`None` : pas encore lue).
    at: Option<Instant>,
    /// La dernière lecture a réussi.
    ok: bool,
    /// Échec déjà journalisé (une ligne par panne, pas une toutes les `RETRY`).
    warned: bool,
}

struct Shared {
    nets: Vec<Cidr>,
    container: String,
    inspect: Inspect,
    own_address: OwnAddress,
    snap: watch::Sender<Snapshot>,
    /// Réveille la tâche de relecture : un pair du réseau n'est pas l'adresse connue.
    wake: Notify,
    /// Adresses déjà testées par `own_address`, avec l'heure du test (refait après `TTL`).
    own: Mutex<HashMap<IpAddr, (bool, Instant)>>,
    /// Pannes de Docker journalisées (lu par les tests).
    warnings: AtomicUsize,
}

impl Shared {
    fn is_own(&self, peer: IpAddr) -> bool {
        let mut cache = self.own.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((own, at)) = cache.get(&peer) {
            if at.elapsed() < TTL {
                return *own;
            }
        }
        match (self.own_address)(peer) {
            Some(own) => {
                if cache.len() >= OWN_CACHE_MAX {
                    cache.clear();
                }
                cache.insert(peer, (own, Instant::now()));
                own
            }
            // indéterminé : pas cru, et retesté à la prochaine requête
            None => true,
        }
    }

    async fn read(&self) {
        let out = match tokio::time::timeout(DOCKER_TIMEOUT, (self.inspect)()).await {
            Ok(r) => r,
            Err(_) => Err("délai dépassé".to_string()),
        };
        let prev = self.snap.borrow().clone();
        let mut next = Snapshot {
            at: Some(Instant::now()),
            ..prev.clone()
        };
        match out {
            Ok(ips) => {
                if !prev.ok || ips != prev.ips {
                    info!(task = "web", container = %self.container, ips = ?ips, "adresse du proxy de confiance");
                }
                next.ips = ips;
                next.ok = true;
                next.warned = false;
            }
            Err(e) => {
                if !prev.warned {
                    warn!(task = "web", container = %self.container, error = %e, "docker inspect en échec : X-Forwarded-For de [web] trusted_proxies gardé pour les limites par adresse, plus de session automatique de la maison");
                    self.warnings.fetch_add(1, Ordering::Relaxed);
                    next.warned = true;
                }
                next.ok = false;
            }
        }
        self.snap.send_replace(next);
    }
}

/// Tâche de relecture : toutes les `TTL`, plus tôt (au plus toutes les `RETRY`) après un réveil.
async fn refresher(sh: Arc<Shared>) {
    // un pair inconnu s'est présenté depuis la dernière lecture
    let mut miss = false;
    loop {
        let age = sh.snap.borrow().at.map(|t| t.elapsed());
        if should_refresh(age, !miss) {
            sh.read().await;
            miss = false;
            continue;
        }
        let wait = if miss { RETRY } else { TTL }.saturating_sub(age.unwrap_or_default());
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = sh.wake.notified(), if !miss => miss = true,
        }
    }
}

pub struct ProxyTrust {
    sh: Arc<Shared>,
    started: OnceLock<()>,
}

impl ProxyTrust {
    /// Les entrées invalides sont déjà refusées par `Config::validate` ; ignorées ici par sûreté.
    pub fn new(cfg: &homelab_core::config::Web) -> Self {
        let container = cfg.trusted_proxy_container.trim().to_string();
        Self::with(
            cfg.trusted_proxies
                .iter()
                .filter_map(|n| Cidr::parse(n).ok())
                .collect(),
            container.clone(),
            docker_inspect(container),
            Arc::new(own_address),
        )
    }

    fn with(nets: Vec<Cidr>, container: String, inspect: Inspect, own_address: OwnAddress) -> Self {
        Self {
            sh: Arc::new(Shared {
                nets,
                container,
                inspect,
                own_address,
                snap: watch::Sender::new(Snapshot::default()),
                wake: Notify::new(),
                own: Mutex::new(HashMap::new()),
                warnings: AtomicUsize::new(0),
            }),
            started: OnceLock::new(),
        }
    }

    /// Lance la tâche de relecture, une fois (rien sans conteneur nommé, ni hors d'un runtime tokio).
    pub fn start(&self) {
        if self.sh.container.is_empty() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.started.get_or_init(|| {
            tokio::spawn(refresher(self.sh.clone()));
        });
    }

    /// `peer` : adresse TCP (`ConnectInfo`) ; absente (jamais en service), la requête n'est ni locale ni relayée.
    pub async fn client(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Client {
        let peer = peer.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let hop = last_hop(headers);
        // sans `X-Forwarded-For`, la confiance ne change rien
        let trust = match hop {
            Some(_) => self.trust(peer).await,
            None => Trust::Ignored,
        };
        resolve(peer, hop, trust)
    }

    async fn trust(&self, peer: IpAddr) -> Trust {
        let sh = &self.sh;
        let peer = peer.to_canonical();
        // filtre gratuit d'abord : un pair hors réseau ou en boucle locale ne coûte rien
        if proxy_trust(peer, &sh.nets, false, None) == Trust::Ignored || sh.is_own(peer) {
            return Trust::Ignored;
        }
        if sh.container.is_empty() {
            return Trust::Network;
        }
        self.start();
        // première lecture pas encore faite (démarrage) : on l'attend, bornée par le délai de Docker
        if sh.snap.borrow().at.is_none() {
            let mut rx = sh.snap.subscribe();
            let first = rx.wait_for(|s| s.at.is_some());
            let _ = tokio::time::timeout(DOCKER_TIMEOUT + Duration::from_secs(1), first).await;
        }
        let trust = {
            let s = sh.snap.borrow();
            proxy_trust(peer, &sh.nets, false, s.ok.then_some(s.ips.as_slice()))
        };
        if trust != Trust::Confirmed {
            // NPM recréé, ou Docker muet : relire plus tôt (la tâche s'en tient à une lecture par `RETRY`)
            sh.wake.notify_one();
        }
        trust
    }
}

/// Pour les tests de la couche d'administration : Docker et les adresses de l'hôte simulés.
#[cfg(test)]
pub mod testing {
    use super::*;

    /// Délai de relecture quand un pair du réseau est inconnu.
    pub const RETRY: Duration = super::RETRY;

    /// Adresses du conteneur proxy rendues par le faux Docker (`Err` : Docker muet), et lectures faites.
    #[derive(Clone)]
    pub struct FakeDocker {
        pub ips: Arc<Mutex<Result<Vec<IpAddr>, String>>>,
        pub reads: Arc<AtomicUsize>,
    }

    impl FakeDocker {
        pub fn answering(ips: &[&str]) -> Self {
            Self {
                ips: Arc::new(Mutex::new(Ok(ips
                    .iter()
                    .map(|s| s.parse().unwrap())
                    .collect()))),
                reads: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub fn silent() -> Self {
            let d = Self::answering(&[]);
            d.set(Err("Cannot connect to the Docker daemon".into()));
            d
        }

        pub fn set(&self, r: Result<Vec<IpAddr>, String>) {
            *self.ips.lock().unwrap() = r;
        }

        pub fn reads(&self) -> usize {
            self.reads.load(Ordering::Relaxed)
        }
    }

    /// `nets` : `[web] trusted_proxies` ; `container` vide = pas de Docker ; `host` : adresses de l'hôte.
    pub fn proxy(nets: &[&str], container: &str, docker: &FakeDocker, host: &[&str]) -> ProxyTrust {
        let d = docker.clone();
        let inspect: Inspect = Arc::new(move || {
            d.reads.fetch_add(1, Ordering::Relaxed);
            let r = d.ips.lock().unwrap().clone();
            Box::pin(async move { r })
        });
        let host: Vec<IpAddr> = host.iter().map(|s| s.parse().unwrap()).collect();
        ProxyTrust::with(
            nets.iter().map(|n| Cidr::parse(n).unwrap()).collect(),
            container.to_string(),
            inspect,
            Arc::new(move |ip| Some(ip.is_loopback() || host.contains(&ip))),
        )
    }

    impl ProxyTrust {
        /// Pannes de Docker journalisées.
        pub fn warnings(&self) -> usize {
            self.sh.warnings.load(Ordering::Relaxed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{proxy, FakeDocker};
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
    fn the_host_is_never_a_proxy() {
        let mut nets = docker_net();
        nets.push(Cidr::parse("127.0.0.0/8").unwrap());
        nets.push(Cidr::parse("::1").unwrap());
        assert_eq!(
            proxy_trust(ip("127.0.0.1"), &nets, false, None),
            Trust::Ignored
        );
        assert_eq!(proxy_trust(ip("::1"), &nets, false, None), Trust::Ignored);
        assert_eq!(
            proxy_trust(ip("::ffff:127.0.0.1"), &nets, false, None),
            Trust::Ignored
        );
        assert_eq!(
            proxy_trust(
                ip("0.0.0.0"),
                &[Cidr::parse("0.0.0.0/0").unwrap()],
                false,
                None
            ),
            Trust::Ignored
        );
        // 172.18.0.1 : l'hôte sur le pont Docker, dans le réseau mais à lui
        assert_eq!(
            proxy_trust(ip("172.18.0.1"), &nets, true, None),
            Trust::Ignored
        );
        let npm = [ip("172.18.0.1")];
        assert_eq!(
            proxy_trust(ip("172.18.0.1"), &nets, true, Some(&npm)),
            Trust::Ignored
        );
    }

    #[test]
    fn own_addresses_are_detected_by_binding() {
        assert_eq!(own_address(ip("127.0.0.1")), Some(true));
        // TEST-NET-3 : jamais configurée sur une machine
        assert_eq!(own_address(ip("203.0.113.254")), Some(false));
    }

    #[test]
    fn only_npm_is_confirmed_when_docker_knows_it() {
        let nets = docker_net();
        let npm = [ip("172.18.0.19")];
        assert_eq!(
            proxy_trust(ip("172.18.0.19"), &nets, false, Some(&npm)),
            Trust::Confirmed
        );
        assert_eq!(
            proxy_trust(ip("::ffff:172.18.0.19"), &nets, false, Some(&npm)),
            Trust::Confirmed
        );
        // un autre conteneur du réseau (Guacamole, Homarr…)
        assert_eq!(
            proxy_trust(ip("172.18.0.5"), &nets, false, Some(&npm)),
            Trust::Ignored
        );
        // NPM arrêté : personne
        assert_eq!(
            proxy_trust(ip("172.18.0.19"), &nets, false, Some(&[])),
            Trust::Ignored
        );
        // Docker muet : le réseau, sans confirmation (jamais d'enfermement dehors, jamais la maison)
        assert_eq!(
            proxy_trust(ip("172.18.0.5"), &nets, false, None),
            Trust::Network
        );
        // hors réseau : jamais, même avec l'adresse du conteneur
        assert_eq!(
            proxy_trust(ip("10.0.0.2"), &nets, false, Some(&[ip("10.0.0.2")])),
            Trust::Ignored
        );
    }

    #[test]
    fn forged_header_from_a_local_process_is_ignored() {
        // `curl -H 'X-Forwarded-For: <IP de la maison>' http://127.0.0.1:8766/…`
        let c = resolve(ip("127.0.0.1"), Some("203.0.113.9".into()), Trust::Ignored);
        assert_eq!(c.ip, "127.0.0.1");
        assert!(!c.via_proxy && !c.proxy_confirmed);
        assert!(!c.local, "relayé : pas la CLI");
        // la CLI : locale, sans en-tête
        let cli = resolve(ip("127.0.0.1"), None, Trust::Ignored);
        assert!(cli.local && !cli.via_proxy);
        assert!(resolve(ip("::1"), None, Trust::Ignored).local);
    }

    #[test]
    fn npm_hop_is_the_client() {
        let c = resolve(
            ip("172.18.0.19"),
            Some("198.51.100.7".into()),
            Trust::Confirmed,
        );
        assert_eq!(
            c,
            Client {
                ip: "198.51.100.7".into(),
                via_proxy: true,
                proxy_confirmed: true,
                local: false
            }
        );
        // repli (Docker muet) : même adresse, sans confirmation
        let net = resolve(
            ip("172.18.0.5"),
            Some("198.51.100.7".into()),
            Trust::Network,
        );
        assert_eq!(net.ip, "198.51.100.7");
        assert!(net.via_proxy && !net.proxy_confirmed);
        // un conteneur quelconque : son adresse, jamais l'en-tête
        let other = resolve(
            ip("172.18.0.5"),
            Some("198.51.100.7".into()),
            Trust::Ignored,
        );
        assert_eq!(other.ip, "172.18.0.5");
        assert!(!other.via_proxy && !other.local);
        // NPM sans en-tête (n'arrive pas) : adresse du pair, ni maison ni CLI
        let bare = resolve(ip("172.18.0.19"), None, Trust::Confirmed);
        assert_eq!(bare.ip, "172.18.0.19");
        assert!(!bare.via_proxy && !bare.proxy_confirmed && !bare.local);
        // IPv4 vue en IPv6 : journalisée en IPv4
        assert_eq!(
            resolve(ip("::ffff:10.1.2.3"), None, Trust::Ignored).ip,
            "10.1.2.3"
        );
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

    const NET: &str = "172.18.0.0/16";

    async fn who(p: &ProxyTrust, peer: &str, hop: &str) -> Client {
        p.client(Some(ip(peer)), &xff(&[hop])).await
    }

    #[tokio::test(start_paused = true)]
    async fn docker_is_read_in_the_background_and_npm_is_found_again_after_a_recreation() {
        let docker = FakeDocker::answering(&["172.18.0.19"]);
        let p = proxy(&[NET], "npm", &docker, &["172.18.0.1"]);
        // première requête : attend la première lecture, puis NPM est confirmé
        let c = who(&p, "172.18.0.19", "198.51.100.7").await;
        assert!(c.proxy_confirmed, "{c:?}");
        assert_eq!(c.ip, "198.51.100.7");
        assert_eq!(docker.reads(), 1);
        // un autre conteneur : son adresse, et une seule relecture par `RETRY` malgré la rafale
        for _ in 0..20 {
            let c = who(&p, "172.18.0.5", "198.51.100.7").await;
            assert_eq!(c.ip, "172.18.0.5");
            assert!(!c.via_proxy);
        }
        tokio::time::sleep(RETRY + Duration::from_secs(1)).await;
        assert_eq!(docker.reads(), 2, "une relecture après le premier inconnu");
        // NPM recréé : nouvelle adresse, reconnue après `RETRY`
        docker.set(Ok(vec![ip("172.18.0.23")]));
        let c = who(&p, "172.18.0.23", "198.51.100.7").await;
        assert!(!c.via_proxy, "pas encore relue");
        tokio::time::sleep(RETRY + Duration::from_secs(1)).await;
        let c = who(&p, "172.18.0.23", "198.51.100.7").await;
        assert!(c.proxy_confirmed, "{c:?}");
        assert!(!who(&p, "172.18.0.19", "198.51.100.7").await.via_proxy);
        // au calme : une lecture par `TTL`
        let before = docker.reads();
        tokio::time::sleep(TTL * 3 + Duration::from_secs(1)).await;
        assert!(docker.reads() - before <= 4, "{}", docker.reads() - before);
    }

    #[tokio::test(start_paused = true)]
    async fn docker_silent_keeps_the_network_without_confirmation_and_warns_once() {
        let docker = FakeDocker::silent();
        let p = proxy(&[NET], "npm", &docker, &["172.18.0.1"]);
        let c = who(&p, "172.18.0.19", "198.51.100.7").await;
        assert_eq!(c.ip, "198.51.100.7", "limites par adresse gardées");
        assert!(c.via_proxy && !c.proxy_confirmed);
        // l'hôte lui-même n'est jamais cru, même en repli
        let host = who(&p, "172.18.0.1", "203.0.113.9").await;
        assert_eq!(host.ip, "172.18.0.1");
        assert!(!host.via_proxy);
        for _ in 0..5 {
            who(&p, "172.18.0.19", "198.51.100.7").await;
            tokio::time::sleep(RETRY + Duration::from_secs(1)).await;
        }
        assert!(docker.reads() >= 3, "Docker relu pendant la panne");
        assert_eq!(p.warnings(), 1, "une ligne par panne");
        // Docker revient : NPM confirmé (relu dans les `RETRY` qui suivent une requête de NPM non confirmée),
        // et une nouvelle panne sera de nouveau signalée
        docker.set(Ok(vec![ip("172.18.0.19")]));
        assert!(!who(&p, "172.18.0.19", "198.51.100.7").await.proxy_confirmed);
        tokio::time::sleep(RETRY + Duration::from_secs(1)).await;
        assert!(who(&p, "172.18.0.19", "198.51.100.7").await.proxy_confirmed);
        docker.set(Err("no such object: npm".into()));
        who(&p, "172.18.0.5", "198.51.100.7").await;
        tokio::time::sleep(RETRY + Duration::from_secs(1)).await;
        assert_eq!(p.warnings(), 2);
    }

    #[tokio::test]
    async fn without_a_container_docker_is_never_called() {
        let docker = FakeDocker::answering(&["172.18.0.19"]);
        let p = proxy(&[NET], "", &docker, &["172.18.0.1"]);
        let c = who(&p, "172.18.0.5", "198.51.100.7").await;
        assert!(c.via_proxy && !c.proxy_confirmed);
        assert!(!who(&p, "172.18.0.1", "198.51.100.7").await.via_proxy);
        assert_eq!(docker.reads(), 0);
    }
}
