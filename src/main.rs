use anyhow::Result;
use hotwatch::{EventKind, Hotwatch};
use hypixel::{ApiHypixelData, HypixelPlayer};
use log::{error, trace, warn};
use regex::Regex;
use reqwest::Client;
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, sync::Arc};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    runtime::Runtime,
};

type Uuid = String;

mod hypixel;

#[derive(Deserialize, Serialize, Clone)]
struct Config {
    #[serde(rename = "log-path")]
    log_path: String,
    #[serde(rename = "api-key")]
    api_key: String,
    #[serde(rename = "quit-level")]
    quit_level: i32,
}

impl std::default::Default for Config {
    fn default() -> Self {
        let mut log_path = dirs::home_dir().unwrap();
        #[cfg(target_os = "windows")]
        {
            log_path.push("AppData");
            log_path.push("Roaming");
        }
        log_path.push(".minecraft");
        log_path.push("logs");

        Config {
            log_path: log_path.display().to_string(),
            api_key: "INSERT_API_KEY_HERE".to_string(),
            quit_level: 130,
        }
    }
}

#[derive(Deserialize)]
struct Player {
    name: String,
    id: String,
}

const CONFIG_PATH: &str = "config.toml";

async fn read_config() -> Result<Config> {
    let exists = matches!(fs::try_exists("config.toml").await, Ok(true));

    if !exists {
        trace!("Creating config file at {CONFIG_PATH}");
        let mut f = File::create(CONFIG_PATH).await?;

        trace!("Generating default config");
        let config = Config::default();
        let config_str = toml::to_string(&config)?;
        f.write_all(config_str.as_bytes()).await;
    }

    let config_str = std::fs::read_to_string(CONFIG_PATH)?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    simple_logger::SimpleLogger::new().env().init().unwrap();

    let config = Arc::new(read_config().await?);
    let rt = Runtime::new()?;
    let rt = Arc::new(rt);

    let mut hotwatch = Hotwatch::new()?;
    hotwatch.watch(config.log_path.clone(), move |event| {
        if let EventKind::Modify(_) = event.kind {
            let log = std::fs::read_to_string(config.log_path.clone()).unwrap();
            let lines: Vec<&str> = log.split("\n").collect();
            let last_line = lines.last().unwrap();
            let player_regex =
                Regex::new(r"\[.*:.*:.*\] \[.* thread\/INFO\]: \[CHAT\] ONLINE: ").unwrap();

            if last_line.starts_with("[CHAT] ONLINE:") {
                let cleaned_line = player_regex.replace_all(last_line, "").replace('\r', "");

                let rt = Arc::clone(&rt);

                let names: Vec<String> = cleaned_line.split(", ").map(|x| x.to_string()).collect();
                // Only god knows why this works.
                let value = config.clone();
                rt.spawn(async move {
                    let players = get_player_uuids(names).await.unwrap();

                    for (_, uuid) in players {
                        let config = value.clone();
                        let hypixel_data = get_hypixel_data(uuid, config)
                            .await
                            .map_err(|e| {
                                error!("Error while getting data from hypixel: {e}");
                            })
                            .unwrap();

                        eprintln!("{:#?}", hypixel_data);
                    }
                });
            }
        }
    })?;

    Ok(())
}

async fn get_player_uuids(names: Vec<String>) -> Result<HashMap<String, Uuid>> {
    let client = Client::new();
    let chunks: Vec<&[String]> = names.chunks(10).collect();

    let mut mojang_players: HashMap<String, Uuid> = HashMap::new();

    for chunk in chunks {
        let body = json!(chunk);
        let response_res = client
            .post("https://api.minecraftservices.com/minecraft/profile/lookup/bulk/byname")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;

        if let Ok(resp) = response_res {
            if !resp.status().is_success() {
                handle_mojang_failure(&client, chunk, &mut mojang_players).await?;
                continue;
            }

            let players: Vec<Player> = resp.json().await?;

            for player in players {
                mojang_players.insert(player.id, player.name);
            }
        }
    }

    Ok(mojang_players)
}

async fn handle_mojang_failure(
    client: &Client,
    chunk: &[String],
    mojang_players: &mut HashMap<String, Uuid>,
) -> Result<()> {
    warn!("There was an error returned from Mojang API.");
    warn!("Retrying using fallback api (api.minetools.eu)...");

    for player in chunk {
        let response_res = client
            .get(format!("https://api.minetools.eu/uuid/{}", player))
            .send()
            .await;

        if let Ok(resp) = response_res {
            let api_player: Player = resp.json().await?;
            mojang_players.insert(api_player.id, api_player.name);
        }
    }

    Ok(())
}

async fn get_hypixel_data(uuid: Uuid, config: Arc<Config>) -> Result<HypixelPlayer> {
    let client = Client::new();
    let response = client
        .get(format!("https://api.hypixel.net/v2/player?uuid={uuid}"))
        .header("API-Key", &config.api_key)
        .send()
        .await;

    if let Ok(resp) = response {
        let hypixel_data: ApiHypixelData = resp.json().await?;
        if hypixel_data.player.is_some() {
            return Ok(HypixelPlayer::from_api(hypixel_data.player.unwrap(), uuid));
        }
    }

    anyhow::bail!("response is not ok");
}
