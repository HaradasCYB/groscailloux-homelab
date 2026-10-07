//! Version du binaire : `git describe --tags --always --dirty` (par exemple `v1.19.0-83-g4400b8b-dirty`), exposée
//! dans `HOMELAB_VERSION`. Le `version = "2.0.0"` de Cargo.toml n'a jamais été incrémenté : `/health` répondait
//! toujours « 2.0.0 » après des dizaines de déploiements. `-dirty` dit que l'arbre de travail avait des
//! modifications non validées (on construit et installe depuis `/opt/homelab` tel quel).
//!
//! Sans git (archive, bac à sable), sans dépôt, ou si git refuse le dossier : repli sur la version de Cargo.toml.

use std::process::Command;

/// Sortie d'une commande git, vide si elle échoue. `safe.directory=*` : un build lancé en root (`sudo ./setup.sh`)
/// sur un dépôt appartenant à `deploy` serait sinon refusé par git (« dubious ownership »).
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(["-c", "safe.directory=*"])
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn main() {
    let fallback = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "inconnue".into());
    let version = git(&["describe", "--tags", "--always", "--dirty"]).unwrap_or(fallback);
    println!("cargo:rustc-env=HOMELAB_VERSION={version}");

    // Relancer quand le dépôt bouge (nouveau commit, changement de branche) ou que le code change : sans cela
    // le drapeau `-dirty` resterait celui du build précédent. Un chemin absent est traité par cargo comme
    // « toujours modifié » : au pire un build.rs de plus, jamais une version périmée.
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(p) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    for dir in [
        "src",
        "../homelab-core/src",
        "../../Cargo.toml",
        "../../Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={dir}");
    }
}
