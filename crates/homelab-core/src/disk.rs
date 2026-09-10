use std::path::Path;

use anyhow::{Context, Result};

/// Pourcentage d'utilisation tel que `df` le calcule : used / (used + avail), arrondi
/// vers le haut (les blocs réservés root ne comptent pas comme disponibles).
pub fn usage_percent(path: &Path) -> Result<u8> {
    let st =
        nix::sys::statvfs::statvfs(path).with_context(|| format!("statvfs {}", path.display()))?;
    let total = st.blocks() as u128;
    let free = st.blocks_free() as u128;
    let avail = st.blocks_available() as u128;
    Ok(percent(total, free, avail))
}

pub fn percent(total: u128, free: u128, avail: u128) -> u8 {
    let used = total.saturating_sub(free);
    let denom = used + avail;
    if denom == 0 {
        return 0;
    }
    ((used * 100).div_ceil(denom)).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_df_rounding() {
        // 793G used, 177G avail sur 969G : df affiche 82%
        assert_eq!(percent(969, 969 - 793, 177), 82);
        assert_eq!(percent(100, 5, 5), 95);
        assert_eq!(percent(100, 1, 1), 99);
        assert_eq!(percent(0, 0, 0), 0);
    }
}
