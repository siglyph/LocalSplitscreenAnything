//! Steam game with the gbe_fork (Goldberg) emulator per instance. Planned for milestone M4:
//! per-instance game folder (junctions), drop-in `steam_api(64).dll`, `steam_settings/` with a
//! distinct steam id and account name per player, LAN discovery over loopback.

use anyhow::{anyhow, Result};

use super::{LaunchRequest, Launched, Provider};

pub struct SteamEmuProvider;

impl Provider for SteamEmuProvider {
    fn name(&self) -> &'static str {
        "steam-emu"
    }

    fn launch(&self, _req: &LaunchRequest<'_>) -> Result<Launched> {
        Err(anyhow!("the steam-emu provider is not implemented yet (milestone M4); use provider = \"steam\" or \"exe\""))
    }
}
