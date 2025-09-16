use std::collections::VecDeque;
use std::fmt;
use std::ops::{Index, IndexMut};
use std::str::FromStr;
use std::time::Duration;
use tokio::time;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize, Serializer};
use serde_with::{DisplayFromStr, serde_as};
use tracing::{error, info};

const API_ENDPOINT_URL: &str = "https://staticinfo.margonem.pl/online/dream.json";
const USERNAME: &str = "Monitorowanie Graczy Online";
const ICON_URL: &str = "https://i.imgur.com/JfXZnjf.png";
const WEBHOOK_URL: &str = "https://discord.com/api/webhooks/1417094385048485908/2QMiWyBr9eFYQ3D1VEmrzlPHst7ar8ObgNikdqSK-1r0ZylVxjjAkU4Z3Ag7YZFdOiVX";
const MAX_INLINE_FIELDS: usize = 3;
/// Time window in which the overtime diff is checked.
const QUEUE_CAPACITY: usize = 5;
/// Max amount of players logged in before treating the amount as enough to notify the server.
const PLAYER_OVERFLOW: u8 = 8;
/// Max amount of players that can differ between the current and lowest from QUEUE_CAPACITY time window.
const MIN_OVERTIME_DIFF: u8 = 6;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    monitor_loop().await?;

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Notif {
    Kic,
    Rene,
    Arcy,
    Zoons,
    Lowka,
    Przyzy,
    Magua,
    Topka,
}

impl fmt::Display for Notif {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Notif::*;

        let data = match self {
            Kic => "Kic",
            Rene => "Rene",
            Arcy => "Arcy",
            Zoons => "Zoons",
            Lowka => "Łowka",
            Przyzy => "Przyzy",
            Magua => "Magua",
            Topka => "Topka",
        };
        f.write_str(data)
    }
}

/// Kic (64..=83)
/// Rene (114)
/// Arcy (144)
/// Zoons (167)
/// Lowka (190)
/// Przyzy (210..=217)
/// Magua (244)
/// Topka (300..=500)
/// Ignore players not falling in the above ranges
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerCounter {
    kic: u8,
    rene: u8,
    arcy: u8,
    zoons: u8,
    lowka: u8,
    przyzy: u8,
    magua: u8,
    topka: u8,
}

impl PlayerCounter {
    // Create a new PlayerCounter from a list of players
    fn new(players: &[PlayerOnlineData]) -> Self {
        let mut counter = Self::default();

        for player in players {
            match player.level {
                64..=83 => counter.kic += 1,
                114 => counter.rene += 1,
                144 => counter.arcy += 1,
                167 => counter.zoons += 1,
                190 => counter.lowka += 1,
                210..=217 => counter.przyzy += 1,
                244 => counter.magua += 1,
                300..=500 => counter.topka += 1,
                _ => {}
            }
        }

        counter
    }

    fn overflow(self) -> bool {
        use Notif::*;

        [Kic, Rene, Arcy, Zoons, Lowka, Przyzy, Magua, Topka]
            .into_iter()
            .map(|variant| self[variant])
            .any(|counter| counter >= PLAYER_OVERFLOW)
    }

    fn get_most_active_ranges(
        &self,
        variants: Vec<Notif>,
        players: &[PlayerOnlineData],
    ) -> Vec<(Notif, u8, Vec<String>)> {
        variants.into_iter().map(|variant| {
            let mut count = 0;
            let players = players
                .iter()
                .filter_map(|player| {
                    if !player.from_range(variant) {
                        return None
                    }

                    count += 1;
                    match count {
                        1..=10 => Some(format!(
                            "{count}. [{}](<https://www.margonem.pl/profile/view,{}#char_{},dream>) ({}{})",
                            player.nick, player.account, player.char_id, player.level, player.prof
                        )),
                        11 => Some("[...]".to_string()),
                        _ => None 
                    }
                })
            .collect();

            (variant, count, players)
        }).collect()
    }
}

impl Index<Notif> for PlayerCounter {
    type Output = u8;
    fn index(&self, index: Notif) -> &Self::Output {
        use Notif::*;

        match index {
            Kic => &self.kic,
            Rene => &self.rene,
            Arcy => &self.arcy,
            Zoons => &self.zoons,
            Lowka => &self.lowka,
            Przyzy => &self.przyzy,
            Magua => &self.magua,
            Topka => &self.topka,
        }
    }
}

impl IndexMut<Notif> for PlayerCounter {
    fn index_mut(&mut self, index: Notif) -> &mut Self::Output {
        use Notif::*;
        match index {
            Kic => &mut self.kic,
            Rene => &mut self.rene,
            Arcy => &mut self.arcy,
            Zoons => &mut self.zoons,
            Lowka => &mut self.lowka,
            Przyzy => &mut self.przyzy,
            Magua => &mut self.magua,
            Topka => &mut self.topka,
        }
    }
}

#[serde_as]
#[derive(Debug, Deserialize, Clone)]
pub struct PlayerOnlineData {
    #[serde(rename = "a")]
    #[serde_as(as = "DisplayFromStr")]
    pub account: u32,
    #[serde(rename = "c")]
    #[serde_as(as = "DisplayFromStr")]
    pub char_id: i32,
    #[serde(rename = "l")]
    #[serde_as(as = "DisplayFromStr")]
    pub level: u16,
    #[serde(rename = "n")]
    pub nick: String,
    #[serde(rename = "p")]
    #[serde_as(as = "DisplayFromStr")]
    pub prof: Profession,
    //#[serde(rename = "r")]
    //#[serde_as(as = "DisplayFromStr")]
    //pub rank: u8,
}

impl PlayerOnlineData {
    fn from_range(&self, range: Notif) -> bool {
        use Notif::*;

        match range {
            Kic => (64..=83).contains(&self.level),
            Rene => self.level == 114,
            Arcy => self.level == 144,
            Zoons => self.level == 167,
            Lowka => self.level == 190,
            Przyzy => (210..=217).contains(&self.level),
            Magua => self.level == 244,
            Topka => (300..=500).contains(&self.level),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Profession {
    BladeDancer = 1,
    Hunter,
    Mage,
    Paladin,
    Tracker,
    #[default]
    Warrior,
}

impl Serialize for Profession {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl fmt::Display for Profession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Profession::*;
        match self {
            Warrior => write!(f, "w"),
            Mage => write!(f, "m"),
            Paladin => write!(f, "p"),
            Hunter => write!(f, "h"),
            Tracker => write!(f, "t"),
            BladeDancer => write!(f, "b"),
        }
    }
}

impl FromStr for Profession {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use Profession::*;

        match s {
            "w" => Ok(Warrior),
            "m" => Ok(Mage),
            "p" => Ok(Paladin),
            "h" => Ok(Hunter),
            "t" => Ok(Tracker),
            "b" => Ok(BladeDancer),
            _ => Err("expected a valid profession character"),
        }
    }
}

#[derive(Debug, Default)]
struct Notified {
    kic: u8,
    rene: u8,
    arcy: u8,
    zoons: u8,
    lowka: u8,
    przyzy: u8,
    magua: u8,
    topka: u8,
}

impl Index<Notif> for Notified {
    type Output = u8;
    fn index(&self, index: Notif) -> &Self::Output {
        use Notif::*;

        match index {
            Kic => &self.kic,
            Rene => &self.rene,
            Arcy => &self.arcy,
            Zoons => &self.zoons,
            Lowka => &self.lowka,
            Przyzy => &self.przyzy,
            Magua => &self.magua,
            Topka => &self.topka,
        }
    }
}

impl IndexMut<Notif> for Notified {
    fn index_mut(&mut self, index: Notif) -> &mut Self::Output {
        use Notif::*;

        match index {
            Kic => &mut self.kic,
            Rene => &mut self.rene,
            Arcy => &mut self.arcy,
            Zoons => &mut self.zoons,
            Lowka => &mut self.lowka,
            Przyzy => &mut self.przyzy,
            Magua => &mut self.magua,
            Topka => &mut self.topka,
        }
    }
}

async fn monitor_loop() -> Result<()> {
    use Notif::*;

    info!("Starting Margonem player monitor...");

    let mut interval = time::interval(Duration::from_secs(60));
    let mut deque = VecDeque::with_capacity(QUEUE_CAPACITY);
    let mut last_notified = Notified::default();

    loop {
        interval.tick().await;

        [Kic, Rene, Arcy, Zoons, Lowka, Przyzy, Magua, Topka]
            .into_iter()
            .for_each(|variant| {
                last_notified[variant] = last_notified[variant].saturating_sub(1);
            });

        info!("Checking for players...");

        let players = fetch_players_data().await?;

        info!("Fetched {} players.", players.len());

        let players_counter = PlayerCounter::new(&players);
        info!("players counter: {players_counter:#?}");

        if deque.len() == deque.capacity() {
            deque.pop_back();
        }
        deque.push_front(players_counter);

        let mut variants: Vec<Notif> = Vec::with_capacity(MAX_INLINE_FIELDS);
        let mut fields_left = MAX_INLINE_FIELDS;

        if players_counter.overflow() {
            info!("OVERFLOW!");
            let mut most_active_ranges = [Kic, Rene, Arcy, Zoons, Lowka, Przyzy, Magua, Topka]
                .into_iter()
                .filter_map(|variant| {
                    if last_notified[variant] != 0 {
                        return None;
                    }
                    
                    (players_counter[variant] >= PLAYER_OVERFLOW)
                        .then_some((variant, players_counter[variant]))
                })
                .collect::<Vec<_>>();

            most_active_ranges.sort_by(|a, b| a.1.cmp(&b.1).reverse());
            most_active_ranges.truncate(MAX_INLINE_FIELDS);
            fields_left -= most_active_ranges.len();

            variants.extend(most_active_ranges.into_iter().map(|(variant, _)| variant));
        }
        if fields_left != 0 {
            let mut from_over_time_overflow = [Kic, Rene, Arcy, Zoons, Lowka, Przyzy, Magua, Topka]
                .into_iter()
                .filter_map(|variant| {
                    if last_notified[variant] != 0 {
                        return None;
                    }

                    // Get the lowest amount of players logged in for the given lvl range in the last 5min.
                    let lowest = deque
                        .iter()
                        .min_by(|a, b| a[variant].cmp(&b[variant]))
                        .unwrap()[variant];
                    let current = players_counter[variant];

                    tracing::info!(
                        "{variant:?}: {} ({current} | {lowest})",
                        current as i32 - lowest as i32
                    );

                    let diff = current.checked_sub(lowest)?;

                    // If the difference between the current player amount and the lowest
                    // one for the given variant is >= MIN_OVERTIME_DIFF we need to notify the server.
                    (diff >= MIN_OVERTIME_DIFF).then_some((variant, diff))
                })
                .collect::<Vec<_>>();

            from_over_time_overflow.retain(|(variant,_)| !variants.contains(variant));
            from_over_time_overflow.sort_by(|a, b| a.1.cmp(&b.1).reverse());
            from_over_time_overflow.truncate(fields_left);

            variants.extend(
                from_over_time_overflow
                    .into_iter()
                    .map(|(variant, _)| variant),
            );
        }
        if !variants.is_empty() {
            variants.iter().for_each(|&variant| last_notified[variant] = 5);
            send_discord_notification(players, players_counter, variants).await?;
        } else {
            info!("No notification due to inactivity.")
        }
    }
}

async fn fetch_players_data() -> Result<Vec<PlayerOnlineData>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client.get(API_ENDPOINT_URL).send().await?;
    let players: Vec<PlayerOnlineData> = response.json().await?;

    Ok(players)
}

#[derive(Debug, Serialize)]
struct DiscordWebhookPayload {
    username: &'static str,
    avatar_url: &'static str,
    embeds: [DiscordEmbed; 1],
}

impl DiscordWebhookPayload {
    fn new(embed: DiscordEmbed) -> Self {
        Self {
            username: USERNAME,
            avatar_url: ICON_URL,
            embeds: [embed],
        }
    }
}

#[derive(Debug, Serialize)]
struct DiscordEmbed {
    title: &'static str,
    color: u32,
    fields: Vec<DiscordField>,
    timestamp: String,
    footer: DiscordEmbedFooter,
}

impl DiscordEmbed {
    fn new(
        current_players: Vec<PlayerOnlineData>,
        current_players_counter: PlayerCounter,
        variants: Vec<Notif>,
    ) -> Self {
        let title = Self::generate_title(&variants);
        let top_active = current_players_counter.get_most_active_ranges(variants, &current_players);
        Self {
            title,
            color: 0x0099FF,
            fields: Self::generate_fields(top_active),
            timestamp: Utc::now().to_rfc3339().to_string(),
            footer: DiscordEmbedFooter::default(),
        }
    }

    fn generate_title(variants: &[Notif]) -> &'static str {
        use Notif::*;

        if variants.len() > 1 {
            return "Wzmożona aktywność na wielu przedziałach!";
        }

        match variants[0] {
            Kic => "Wzmożona aktywność na Kicu!",      // 64..=83
            Rene => "Wzmożona aktywność na Rene!",     // 114
            Arcy => "Wzmożona aktywność na Arcym!",    // 144
            Zoons => "Wzmożona aktywność na Zoonsie!", // 167
            Lowka => "Wzmożona aktywność na Łowce!",   // 190
            Przyzy => "Wzmożona aktywność na Przyzy!", // 210..=217
            Magua => "Wzmożona aktywność na Magule!",  // 244
            Topka => "Wzmożona aktywność na Topce!",   // 300..=500
        }
    }

    fn generate_fields(top_active: Vec<(Notif, u8, Vec<String>)>) -> Vec<DiscordField> {
        top_active.into_iter().map(DiscordField::new).collect()
    }
}

#[derive(Debug, Serialize)]
struct DiscordEmbedFooter {
    text: &'static str,
    icon_url: &'static str,
}

impl Default for DiscordEmbedFooter {
    fn default() -> Self {
        Self {
            text: "discord.gg/dodatki-margonem",
            icon_url: ICON_URL,
        }
    }
}

#[derive(Debug, Serialize)]
struct DiscordField {
    name: String,
    value: String,
    inline: bool,
}

impl DiscordField {
    fn new((variant, count, players): (Notif, u8, Vec<String>)) -> Self {
        Self {
            name: format!("{variant} ({count})"),
            value: players.join("\n"),
            inline: true,
        }
    }
}

async fn send_discord_notification(
    players: Vec<PlayerOnlineData>,
    players_counter: PlayerCounter,
    variants: Vec<Notif>,
) -> Result<()> {
    let embed = DiscordEmbed::new(players, players_counter, variants);
    let payload = DiscordWebhookPayload::new(embed);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let response = client.post(WEBHOOK_URL).json(&payload).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await?;
        error!("Discord webhook failed with status {}: {}", status, text);
        anyhow::bail!("Discord webhook failed: {} - {}", status, text);
    }

    info!("Discord notification sent successfully!");
    Ok(())
}
