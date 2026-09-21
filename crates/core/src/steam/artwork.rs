//! Library artwork for the shortcut, taken from the game's official Steam store assets.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::IoContext;
use crate::{Error, Result};

/// Aniimo's app id on the Steam store; only used to look up artwork.
pub const STORE_APP_ID: u32 = 4126040;

const ASSET_HOST: &str = "https://shared.akamai.steamstatic.com/store_item_assets/";
const ICON_HOST: &str = "https://shared.akamai.steamstatic.com/community_assets/images/apps/";

#[derive(Debug, Default, Deserialize)]
struct Assets {
    asset_url_format: Option<String>,
    library_capsule: Option<String>,
    library_capsule_2x: Option<String>,
    header: Option<String>,
    header_2x: Option<String>,
    library_hero: Option<String>,
    library_hero_2x: Option<String>,
    library_logo: Option<String>,
    library_logo_2x: Option<String>,
    community_icon: Option<String>,
}

/// Files written for a shortcut; `icon` should be set as the shortcut's icon.
#[derive(Debug, Default)]
pub struct Written {
    pub files: Vec<PathBuf>,
    pub icon: Option<PathBuf>,
}

/// Download artwork into `grid_dir` using Steam's naming scheme for `shortcut_appid`.
/// Individual images that fail are skipped; only a failed asset lookup is an error.
pub fn install(agent: &ureq::Agent, grid_dir: &Path, shortcut_appid: u32) -> Result<Written> {
    let assets = fetch_assets(agent)?;
    fs::create_dir_all(grid_dir).io_ctx(|| format!("creating {}", grid_dir.display()))?;
    let format = assets.asset_url_format.clone().unwrap_or_default();
    let url = |file: &Option<String>| {
        file.as_ref()
            .filter(|_| !format.is_empty())
            .map(|f| format!("{ASSET_HOST}{}", format.replace("${FILENAME}", f)))
    };

    let wanted = [
        (url(&assets.library_capsule_2x).or(url(&assets.library_capsule)), format!("{shortcut_appid}p.jpg")),
        (url(&assets.header_2x).or(url(&assets.header)), format!("{shortcut_appid}.jpg")),
        (url(&assets.library_hero_2x).or(url(&assets.library_hero)), format!("{shortcut_appid}_hero.jpg")),
        (url(&assets.library_logo_2x).or(url(&assets.library_logo)), format!("{shortcut_appid}_logo.png")),
    ];

    let mut written = Written::default();
    for (src, name) in wanted {
        let Some(src) = src else { continue };
        let dest = grid_dir.join(name);
        if save(agent, &src, &dest).is_ok() {
            written.files.push(dest);
        }
    }
    if let Some(hash) = &assets.community_icon {
        let dest = grid_dir.join(format!("{shortcut_appid}_icon.jpg"));
        if save(agent, &format!("{ICON_HOST}{STORE_APP_ID}/{hash}.jpg"), &dest).is_ok() {
            written.icon = Some(dest.clone());
            written.files.push(dest);
        }
    }
    Ok(written)
}

fn fetch_assets(agent: &ureq::Agent) -> Result<Assets> {
    #[derive(Deserialize)]
    struct Resp {
        response: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        #[serde(default)]
        store_items: Vec<Item>,
    }
    #[derive(Deserialize)]
    struct Item {
        #[serde(default)]
        assets: Assets,
    }

    let input = serde_json::json!({
        "ids": [{ "appid": STORE_APP_ID }],
        "context": { "language": "english", "country_code": "US" },
        "data_request": { "include_assets": true },
    });
    let resp: Resp = agent
        .get("https://api.steampowered.com/IStoreBrowseService/GetItems/v1")
        .query("input_json", input.to_string())
        .call()?
        .body_mut()
        .read_json()?;
    resp.response
        .store_items
        .into_iter()
        .next()
        .map(|i| i.assets)
        .ok_or_else(|| Error::Steam("Steam store returned no artwork for Aniimo".into()))
}

fn save(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<()> {
    let mut bytes = Vec::new();
    agent
        .get(url)
        .call()?
        .body_mut()
        .as_reader()
        .take(20 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .io_ctx(|| format!("downloading {url}"))?;
    fs::write(dest, bytes).io_ctx(|| format!("writing {}", dest.display()))
}
