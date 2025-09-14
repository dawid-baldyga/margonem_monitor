use anyhow::Result;
use chrono::DateTime;
use futures_signals::signal::Mutable;
use futures_signals::signal_map::MutableBTreeMap;
use reqwest::header::LAST_MODIFIED;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{error, info, warn};

pub type OtherId = u32;

#[derive(Debug)]
pub struct PlayersOnline {
    /// Online players map where the key is the character id of a player.
    players_map: MutableBTreeMap<OtherId, PlayerOnlineData>,
    last_updated: Mutable<String>,
}

#[serde_as]
#[derive(Debug, Deserialize, Clone)]
pub struct PlayerOnlineData {
    #[serde(rename = "a")]
    #[serde_as(as = "DisplayFromStr")]
    pub account: u32,
    #[serde(rename = "c")]
    #[serde_as(as = "DisplayFromStr")]
    pub char_id: OtherId,
    #[serde(rename = "l")]
    #[serde_as(as = "DisplayFromStr")]
    pub level: u16,
    #[serde(rename = "n")]
    pub nick: String,
    // Commented out fields as per requirements
    //#[serde(rename = "p")]
    //#[serde_as(as = "DisplayFromStr")]
    //pub prof: Profession,
    //#[serde(rename = "r")]
    //#[serde_as(as = "DisplayFromStr")]
    //pub rank: u8,
}

#[derive(Debug, Serialize)]
struct DiscordWebhookPayload {
    content: String,
    embeds: Vec<DiscordEmbed>,
}

#[derive(Debug, Serialize)]
struct DiscordEmbed {
    title: String,
    description: String,
    color: u32,
    fields: Vec<DiscordField>,
    timestamp: String,
}

#[derive(Debug, Serialize)]
struct DiscordField {
    name: String,
    value: String,
    inline: bool,
}

struct LevelRangeInfo {
    range: std::ops::RangeInclusive<u16>,
    players: Vec<PlayerOnlineData>,
}

impl LevelRangeInfo {
    fn new(range: std::ops::RangeInclusive<u16>) -> Self {
        Self {
            range,
            players: Vec::new(),
        }
    }

    fn description(&self) -> String {
        format!("Level {}-{}", self.range.start(), self.range.end())
    }
}

#[derive(Debug)]
struct MonitorState {
    /// Track which ranges had >4 players in the last notification
    last_notified_ranges: HashSet<String>,
    /// Last time a notification was sent
    last_notification_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl MonitorState {
    fn new() -> Self {
        Self {
            last_notified_ranges: HashSet::new(),
            last_notification_time: None,
        }
    }

    fn should_notify(&self, ranges_info: &[LevelRangeInfo]) -> bool {
        // Check if we should send a notification
        // Send if:
        // 1. This is the first check (no previous notification)
        // 2. The set of ranges with >4 players has changed
        // 3. It's been more than 30 minutes since the last notification (to avoid spam)
        
        let current_ranges: HashSet<String> = ranges_info
            .iter()
            .filter(|info| info.players.len() > 4)
            .map(|info| info.description())
            .collect();

        if current_ranges.is_empty() {
            return false;
        }

        if self.last_notification_time.is_none() {
            return true;
        }

        // Check if ranges have changed
        if current_ranges != self.last_notified_ranges {
            return true;
        }

        // Re-notify after 30 minutes if conditions persist
        if let Some(last_time) = self.last_notification_time {
            let elapsed = chrono::Utc::now() - last_time;
            if elapsed.num_minutes() >= 30 {
                return true;
            }
        }

        false
    }

    fn update(&mut self, ranges_info: &[LevelRangeInfo]) {
        self.last_notified_ranges = ranges_info
            .iter()
            .filter(|info| info.players.len() > 4)
            .map(|info| info.description())
            .collect();
        self.last_notification_time = Some(chrono::Utc::now());
    }
}

async fn fetch_players_data() -> Result<(Vec<PlayerOnlineData>, String)> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    
    let response = client
        .get("https://staticinfo.margonem.pl/online/dream.json")
        .send()
        .await?;

    // Extract last-modified header
    let last_modified = response
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Unknown")
        .to_string();

    let players: Vec<PlayerOnlineData> = response.json().await?;
    
    Ok((players, last_modified))
}

fn analyze_level_ranges(players: &[PlayerOnlineData]) -> Vec<LevelRangeInfo> {
    let ranges = vec![
        64..=83,
        144..=167,
        167..=190,
    ];

    ranges
        .into_iter()
        .map(|range| {
            let mut info = LevelRangeInfo::new(range.clone());
            info.players = players
                .iter()
                .filter(|p| range.contains(&p.level))
                .cloned()
                .collect();
            info
        })
        .collect()
}

async fn send_discord_notification(
    ranges_info: &[LevelRangeInfo],
    last_updated: &str,
) -> Result<()> {
    let webhook_url = "https://discord.com/api/webhooks/1416739873590874112/TfWPIqX_LXUu7qDY8KaBNY5DHrXpkoZ5_XgiBeDA18UoWMY-vKNHTV-RwJnlYxJabA3g";
    
    let mut fields = Vec::new();
    let mut total_players = 0;
    
    for info in ranges_info {
        if info.players.len() > 4 {
            total_players += info.players.len();
            
            // Get player nicknames (max 10 to avoid too long messages)
            let player_list = info.players
                .iter()
                .take(10)
                .map(|p| format!("{} (Lv.{})", p.nick, p.level))
                .collect::<Vec<_>>()
                .join("\n");
            
            let value = if info.players.len() > 10 {
                format!("{}\n... and {} more", player_list, info.players.len() - 10)
            } else {
                player_list
            };
            
            fields.push(DiscordField {
                name: format!("{} ({} players)", info.description(), info.players.len()),
                value,
                inline: false,
            });
        }
    }
    
    if fields.is_empty() {
        info!("No level ranges have more than 4 players. Skipping Discord notification.");
        return Ok(());
    }
    
    let embed = DiscordEmbed {
        title: "🎮 Margonem Player Alert".to_string(),
        description: format!(
            "Multiple level ranges have more than 4 players online!\nTotal: {} players in monitored ranges",
            total_players
        ),
        color: 0x00ff00, // Green color
        fields,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    
    let payload = DiscordWebhookPayload {
        content: format!("**Player Activity Alert** - Last updated: {}", last_updated),
        embeds: vec![embed],
    };
    
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    
    let response = client
        .post(webhook_url)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await?;
        error!("Discord webhook failed with status {}: {}", status, text);
        anyhow::bail!("Discord webhook failed: {} - {}", status, text);
    }
    
    info!("Discord notification sent successfully!");
    Ok(())
}

async fn monitor_loop(state: Arc<tokio::sync::Mutex<MonitorState>>) {
    let mut interval = time::interval(Duration::from_secs(60));
    let mut consecutive_errors = 0;
    
    loop {
        interval.tick().await;
        
        info!("Running monitor check...");
        
        match fetch_players_data().await {
            Ok((players, last_modified)) => {
                consecutive_errors = 0;
                info!("Fetched {} players. Last modified: {}", players.len(), last_modified);
                
                // Create PlayersOnline structure
                let _players_online = PlayersOnline {
                    players_map: {
                        let map = MutableBTreeMap::new();
                        for player in &players {
                            map.lock_mut().insert(player.char_id, player.clone());
                        }
                        map
                    },
                    last_updated: Mutable::new(last_modified.clone()),
                };
                
                // Analyze level ranges
                let ranges_info = analyze_level_ranges(&players);
                
                // Log analysis results
                for info in &ranges_info {
                    let count = info.players.len();
                    if count > 0 {
                        info!(
                            "Range {}: {} players",
                            info.description(),
                            count
                        );
                        
                        if count > 4 {
                            warn!(
                                "Range {} has more than 4 players ({})!",
                                info.description(),
                                count
                            );
                        }
                    }
                }
                
                // Check if we should send a notification
                let mut state_guard = state.lock().await;
                if state_guard.should_notify(&ranges_info) {
                    match send_discord_notification(&ranges_info, &last_modified).await {
                        Ok(_) => {
                            state_guard.update(&ranges_info);
                            info!("Notification sent and state updated");
                        }
                        Err(e) => {
                            error!("Failed to send Discord notification: {}", e);
                        }
                    }
                } else {
                    info!("Conditions unchanged or too soon to notify again");
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                error!("Failed to fetch player data (attempt {}): {}", consecutive_errors, e);
                
                // If we've had 5 consecutive errors, wait longer before retrying
                if consecutive_errors >= 5 {
                    warn!("Too many consecutive errors. Waiting 5 minutes before retrying...");
                    time::sleep(Duration::from_secs(300)).await;
                    consecutive_errors = 0;
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Starting Margonem player monitor...");
    info!("Monitor will check every 60 seconds");
    
    // Create shared state
    let state = Arc::new(tokio::sync::Mutex::new(MonitorState::new()));
    
    // Run initial check immediately
    info!("Performing initial check...");
    match fetch_players_data().await {
        Ok((players, last_modified)) => {
            info!("Initial fetch: {} players. Last modified: {}", players.len(), last_modified);
            
            let ranges_info = analyze_level_ranges(&players);
            
            for info in &ranges_info {
                let count = info.players.len();
                if count > 0 {
                    info!("Range {}: {} players", info.description(), count);
                }
            }
            
            // Send initial notification if thresholds are met
            if ranges_info.iter().any(|info| info.players.len() > 4) {
                let mut state_guard = state.lock().await;
                match send_discord_notification(&ranges_info, &last_modified).await {
                    Ok(_) => {
                        state_guard.update(&ranges_info);
                        info!("Initial notification sent");
                    }
                    Err(e) => {
                        error!("Failed to send initial Discord notification: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed initial fetch: {}", e);
            warn!("Continuing with monitor loop despite initial failure");
        }
    }
    
    // Start the monitoring loop
    info!("Starting monitoring loop...");
    monitor_loop(state).await;
    
    Ok(())
}