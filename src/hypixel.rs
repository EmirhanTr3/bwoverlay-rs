#[allow(dead_code)]
use serde_derive::Deserialize;

use crate::Uuid;

pub const BASE: f32 = 10000.0;
pub const GROWTH: f32 = 2500.0;

const REVERSE_PQ_PREFIX: f32 = -(BASE - 0.5 * GROWTH) / GROWTH;
const REVERSE_CONST: f32 = REVERSE_PQ_PREFIX * REVERSE_PQ_PREFIX;
const GROWTH_DIVIDES_2: f32 = 2.0 / GROWTH;

#[derive(Deserialize, Debug)]
pub struct HypixelPlayer {
    pub name: String,
    pub uuid: Uuid,
    pub rank: String,
    pub network_xp: i32,
    pub network_level: i32,
    pub level: i32,
    pub winstreak: i32,
    pub fkdr: f32,
    pub wlr: f32,
    pub final_kills: i32,
    pub wins: i32,
    pub bed_break: i32,
}

impl HypixelPlayer {
    pub fn from_api(raw_info: ApiHypixelPlayer, player_uuid: Uuid) -> Self {
        let stats = raw_info.stats.as_ref();
        let bedwars = stats.and_then(|s| s.bedwars.as_ref());
        let achievements = raw_info.achievements.as_ref();

        let (final_kills, final_deaths) = (
            bedwars.and_then(|b| b.final_kills_bedwars).unwrap_or(-1),
            bedwars.and_then(|b| b.final_deaths_bedwars).unwrap_or(-1),
        );
        let (wins, losses) = (
            bedwars.and_then(|b| b.wins_bedwars).unwrap_or(-1),
            bedwars.and_then(|b| b.losses_bedwars).unwrap_or(-1),
        );

        HypixelPlayer {
            name: raw_info.name,
            uuid: player_uuid,
            rank: match raw_info.monthly_package_rank.as_deref() {
                Some("SUPERSTAR") => "MVP++".to_string(),
                _ => raw_info
                    .new_package_rank
                    .as_deref()
                    .unwrap_or("Default")
                    .replace("_PLUS", "+"),
            },
            network_xp: raw_info.network_xp.unwrap_or(0),
            network_level: calculate_level(raw_info.network_xp.unwrap_or(-1) as f32).round() as i32,
            level: achievements.and_then(|a| a.bedwars_level).unwrap_or(-1),
            winstreak: bedwars.and_then(|b| b.winstreak).unwrap_or(-1),
            fkdr: final_kills as f32 / final_deaths as f32,
            wlr: wins as f32 / losses as f32,
            final_kills,
            wins: bedwars.and_then(|b| b.wins_bedwars).unwrap_or(-1),
            bed_break: bedwars.and_then(|b| b.beds_broken_bedwars).unwrap_or(-1),
        }
    }
}

#[derive(Deserialize)]
pub struct ApiHypixelData {
    pub player: Option<ApiHypixelPlayer>,
}

#[derive(Deserialize, Clone)]
pub struct ApiHypixelPlayer {
    #[serde(rename = "displayname")]
    name: String,
    #[serde(rename = "monthlyPackageRank")]
    monthly_package_rank: Option<String>,
    #[serde(rename = "newPackageRank")]
    new_package_rank: Option<String>,
    #[serde(rename = "networkExp")]
    network_xp: Option<i32>,
    achievements: Option<ApiAchievements>,
    stats: Option<ApiStats>,
}

#[derive(Deserialize, Clone)]
struct ApiAchievements {
    bedwars_level: Option<i32>,
}

#[derive(Deserialize, Clone)]
struct ApiStats {
    #[serde(rename = "Bedwars")]
    bedwars: Option<ApiBedwarsStats>,
}

#[derive(Deserialize, Clone)]
struct ApiBedwarsStats {
    winstreak: Option<i32>,
    final_kills_bedwars: Option<i32>,
    final_deaths_bedwars: Option<i32>,
    wins_bedwars: Option<i32>,
    losses_bedwars: Option<i32>,
    beds_broken_bedwars: Option<i32>,
}

fn calculate_level(exp: f32) -> f32 {
    if exp < 0.0 {
        1.0
    } else {
        (1.0 + REVERSE_PQ_PREFIX + (REVERSE_CONST + GROWTH_DIVIDES_2 * exp).sqrt()).floor()
    }
}
