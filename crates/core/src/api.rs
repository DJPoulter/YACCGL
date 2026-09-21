//! Client for the official FunPlus PC launcher version API (see docs/RESEARCH.md).

use serde::{Deserialize, Deserializer, Serialize};

use crate::{Error, Result};

pub const API_BASE: &str = "https://pc-client-api.funplus.com";
pub const GAME_PROJECT: &str = "worldx_global";

/// One downloadable build of the game client, as described by the API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GamePackage {
    /// Server-side build id. Changes with every release.
    pub id: u64,
    pub version_number: String,
    /// MD5 of the `.7z` archive (lowercase hex).
    pub md5: String,
    pub url: String,
    #[serde(default)]
    pub back_url: String,
    #[serde(default)]
    pub exe_start_name: String,
    /// Approximate total disk usage once the game has fetched its assets.
    #[serde(default, deserialize_with = "lenient_u64")]
    pub package_file_size: Option<u64>,
    #[serde(default)]
    pub ext: PackageExt,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageExt {
    /// Size of the `.7z` download in bytes.
    #[serde(rename = "7z_file_size", default, deserialize_with = "lenient_u64")]
    pub archive_size: Option<u64>,
}

impl GamePackage {
    /// Download URLs in order of preference, with the literal `|` the CDN uses escaped.
    pub fn download_urls(&self) -> Vec<String> {
        [&self.url, &self.back_url]
            .into_iter()
            .filter(|u| !u.is_empty())
            .map(|u| u.replace('|', "%7C"))
            .collect()
    }

    pub fn exe_name(&self) -> &str {
        if self.exe_start_name.is_empty() {
            crate::DEFAULT_EXE
        } else {
            &self.exe_start_name
        }
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    code: i64,
    #[serde(default)]
    msg: String,
    data: Option<T>,
}

#[derive(Deserialize)]
struct VersionList {
    primitive_game: GamePackage,
}

/// Fetch the latest game build.
pub fn latest_game(agent: &ureq::Agent) -> Result<GamePackage> {
    let url = format!("{API_BASE}/api/version/list?game_project={GAME_PROJECT}");
    let env: Envelope<VersionList> = agent.get(&url).call()?.body_mut().read_json()?;
    parse_envelope(env).map(|v| v.primitive_game)
}

fn parse_envelope<T>(env: Envelope<T>) -> Result<T> {
    match (env.code, env.data) {
        (0, Some(data)) => Ok(data),
        (code, _) => Err(Error::Api(format!("code {code}: {}", env.msg))),
    }
}

/// The API sends some sizes as strings and some as numbers, and sometimes `null`.
fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Num(u64),
        Str(String),
    }
    Ok(match Option::<Raw>::deserialize(d)? {
        Some(Raw::Num(n)) => Some(n),
        Some(Raw::Str(s)) => s.trim().parse().ok(),
        None => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed real response from 2026-09-21.
    const SAMPLE: &str = r#"{"code":0,"data":{"platfrom":{"id":2389},"primitive_game":{"id":2402,"md5":"76b8cdf4b45418ea6ef3c8d6404e656e","version_number":"1.0.0.3","version_code":0,"package_name":"","url":"https:\/\/worldx-global-pc-cdn.kingsgroupgames.com\/pc-client-launcher\/worldx_global|primitive\/prod\/package_new\/76b8cdf4b45418ea6ef3c8d6404e656e.7z","back_url":"https:\/\/worldx-global-pc-cdn-backup.kingsgroupgames.com\/pc-client-launcher\/worldx_global|primitive\/prod\/package_new\/76b8cdf4b45418ea6ef3c8d6404e656e.7z","7z_url":"","7z_package_file_size":null,"package_file_size":"40880880831","update_status":1,"exe_start_name":"Aniimo.exe","ext":{"7z_file_size":340386629},"limit_new":0},"ts":1790014540},"msg":"SUCCESS"}"#;

    #[test]
    fn parses_version_list() {
        let env: Envelope<VersionList> = serde_json::from_str(SAMPLE).unwrap();
        let game = parse_envelope(env).unwrap().primitive_game;
        assert_eq!(game.id, 2402);
        assert_eq!(game.version_number, "1.0.0.3");
        assert_eq!(game.package_file_size, Some(40_880_880_831));
        assert_eq!(game.ext.archive_size, Some(340_386_629));
        assert_eq!(game.exe_name(), "Aniimo.exe");
        let urls = game.download_urls();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].contains("worldx_global%7Cprimitive"));
        assert!(urls[1].contains("cdn-backup"));
    }

    #[test]
    fn api_error_is_reported() {
        let env: Envelope<VersionList> =
            serde_json::from_str(r#"{"code":1001,"msg":"bad project","data":null}"#).unwrap();
        assert!(matches!(parse_envelope(env), Err(Error::Api(m)) if m.contains("bad project")));
    }
}
