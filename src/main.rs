use anyhow::Result;
use hotwatch::{EventKind, Hotwatch};
use hypixel::{ApiHypixelData, HypixelPlayer};
use log::{error, info, warn, LevelFilter};
use regex::Regex;
use reqwest::Client;
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    runtime::Runtime,
};
use uuid as uuid_crate;

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
        log_path.push("latest.log");

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
        info!("Creating config file at {CONFIG_PATH}");
        let mut f = File::create(CONFIG_PATH).await?;

        info!("Generating default config");
        let config = Config::default();
        let config_str = toml::to_string(&config)?;
        let _ = f.write_all(config_str.as_bytes()).await;
    }

    let config_str = std::fs::read_to_string(CONFIG_PATH)?;
    let mut config: Config = toml::from_str(&config_str)?;
    let mut log_path = PathBuf::from(&config.log_path);
    if !log_path.ends_with("latest.log") {
        warn!("Log path is not pointing to latest.log, pushing it to path");
        log_path.push("latest.log");
    }
    config.log_path = log_path.to_string_lossy().to_string();

    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    simple_logger::SimpleLogger::new()
        .env()
        .with_level(LevelFilter::Info)
        .init()
        .unwrap();

    let config = Arc::new(read_config().await?);
    let rt = Arc::new(Runtime::new()?);
    let last_processed_line = Arc::new(std::sync::Mutex::new(String::new()));

    let mut hotwatch = Hotwatch::new()?;
    info!("Watching log path: {}", config.log_path);
    hotwatch.watch(config.log_path.clone(), {
        let config = Arc::clone(&config);
        let rt = Arc::clone(&rt);
        let last_processed_line = Arc::clone(&last_processed_line);

        move |event| {
            if let EventKind::Modify(_) = event.kind {
                let log = match std::fs::read_to_string(&config.log_path) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("Error reading log: {e}");
                        return;
                    }
                };

                let last_line = log
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("");
                info!("Last line: {}", last_line);

                // Check for duplicates
                {
                    let mut stored_line = last_processed_line.lock().unwrap();
                    if last_line == *stored_line {
                        return;
                    }
                    *stored_line = last_line.to_string();
                }

                let player_regex = Regex::new(r"\[CHAT\] ONLINE: (.*)").unwrap();

                if player_regex.is_match(last_line) {
                    info!("/who has been executed");
                    let captures = player_regex.captures(last_line).unwrap();
                    let cleaned_line = captures.get(1).unwrap().as_str();
                    info!("Cleaned line: {}", cleaned_line);

                    let names: Vec<String> =
                        cleaned_line.split(", ").map(|x| x.to_string()).collect();
                    info!("Names: {:?}", names);
                    // Only god knows why this works.
                    let value = config.clone();
                    rt.spawn(async move {
                        info!("Getting player uuids");
                        let players = get_player_uuids(names)
                            .await
                            .map_err(|e| {
                                error!("Error while getting player uuids: {e}");
                            })
                            .unwrap();

                        for (uuid, player) in players {
                            info!("Getting hypixel data for {}", uuid);
                            let config = value.clone();
                            info!("UUID for {}: {}", player, uuid);
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
        }
    })?;

    // Keep the program running indefinitely
    tokio::signal::ctrl_c().await?;
    warn!("Received CTRL+C. Closing");

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
    info!("UUID being passed: {uuid}");
    let hypixel_uuid = uuid_crate::Uuid::parse_str(&uuid)
        .map_err(|e| {
            error!("Invalid UUID format: {e}");
        })
        .unwrap();

    let url = format!(
        "https://api.hypixel.net/player?key={}&uuid={}",
        config.api_key, hypixel_uuid
    );

    let client = Client::new();
    let response = client.get(&url).send().await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        error!("Hypixel API returned an error: {}", body);
        return Err(anyhow::anyhow!("Hypixel API error: {}", status));
    }

    let parsed: Result<HypixelPlayer, _> = serde_json::from_str(&body);

    if let Err(e) = &parsed {
        error!(
            "Failed to parse Hypixel API response: {}\nBody: {}",
            e, body
        );
    }

    parsed.map_err(|e| anyhow::anyhow!("Failed to parse Hypixel API response: {}", e))
}

// TODO: uncomment this later and replace the get_hypixel_data function with this one
// async fn get_hypixel_data(uuid: Uuid, config: Arc<Config>) -> Result<HypixelPlayer> {
//     info!("UUID being passed: {uuid}");
//     let hypixel_uuid = uuid_crate::Uuid::parse_str(&uuid).unwrap();
//     info!("About to send: {}", &hypixel_uuid);
//     let client = Client::new();
//     let response = client
//         .get(format!(
//             "https://api.hypixel.net/v2/player?uuid={}",
//             &hypixel_uuid
//         ))
//         .header("API-Key", &config.api_key)
//         // .query(&[("uuid", hypixel_uuid.to_string())])
//         .send()
//         .await;
//
//     match response {
//         Ok(resp) => {
//             if !resp.status().is_success() {
//                 anyhow::bail!("response is not ok: {}", resp.status());
//             }
//             let hypixel_data: ApiHypixelData = resp.json().await?;
//             if hypixel_data.player.is_some() {
//                 return Ok(HypixelPlayer::from_api(hypixel_data.player.unwrap(), uuid));
//             }
//         }
//         Err(e) => {
//             return Err(anyhow::anyhow!(e));
//         }
//     }
//
//     anyhow::bail!("response is not ok");
// }
