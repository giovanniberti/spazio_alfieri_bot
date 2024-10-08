#![feature(iter_array_chunks)]

use crate::crontap::types::{AddSchedule, KeyValue, Timezone};
use crate::crontap::Client;
use anyhow::{anyhow, bail, ensure, Context};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_auth::AuthBearer;
use chrono::{Datelike, Timelike, Utc};
use chrono_tz::Europe;
use hmac::{Hmac, Mac};
use itertools::Itertools;
use migration::{Migrator, MigratorTrait};
use reqwest::Url;
use sea_orm::{
    ActiveModelTrait, ActiveValue, Database, DatabaseConnection, EntityTrait, LoaderTrait,
    ModelTrait, QueryOrder,
};
use serde::Deserialize;
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ParseMode, Recipient};
use teloxide::utils::markdown;
use tokio::task::JoinSet;
use tracing::level_filters::LevelFilter;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::parser::{parse_email_body, DateEntry, NewsletterEntry, ProgrammingEntry};

mod crontap;
mod parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()
        .expect("invalid RUST_LOG environment variable!");

    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_filter(env_filter);

    let subscriber = tracing_subscriber::registry().with(fmt_layer);

    tracing::subscriber::set_global_default(subscriber).expect("unable to set up tracing");

    info!("Starting SpazioAlfieriBot...");

    let bot = Bot::from_env();
    let channel_id = {
        let raw =
            std::env::var("CHANNEL_ID").context("Unable to get environment variable CHANNEL_ID")?;

        ChatId(
            i64::from_str(&raw)
                .with_context(|| format!("Unable to parse channel id '{}' into i64", raw))?,
        )
    };

    let error_chat_id = {
        let raw = std::env::var("ERROR_CHAT_ID")
            .context("Unable to get environment variable ERROR_CHAT_ID")?;

        ChatId(
            i64::from_str(&raw)
                .with_context(|| format!("Unable to parse error chat id '{}' into i64", raw))?,
        )
    };

    let allowed_senders = std::env::var("ALLOWED_SENDERS")
        .context("Unable to get environment variable ALLOWED_SENDERS")?
        .split(",")
        .map(str::to_string)
        .collect();

    let mailgun_webhook_signing_key = std::env::var("MAILGUN_WEBHOOK_SIGNING_KEY")
        .context("Unable to get environment variable MAILGUN_WEBHOOK_SIGNING_KEY")?;

    let update_token = std::env::var("UPDATE_TOKEN")
        .context("Unable to read UPDATE_TOKEN environment variable")?;

    let crontap_client_id = std::env::var("CRONTAP_CLIENT_ID")
        .context("Unable to read CRONTAP_CLIENT_ID environment variable")?;

    let crontap_api_key = std::env::var("CRONTAP_API_KEY")
        .context("Unable to read CRONTAP_API_KEY environment variable")?;

    let crontap_client = Client::new("https://cron.apihustle.com/");

    let host_baseurl = {
        let raw = std::env::var("HOST_BASEURL")
            .context("Unable to read HOST_BASEURL environment variable")?;

        Url::parse(&raw)
            .with_context(|| format!("Unable to parse host baseurl '{}' as URL", raw))?
    };
    let webhook_update_url = host_baseurl
        .join("/update")
        .context("Unable to join update path to host baseurl")?;

    let db_host = std::env::var("POSTGRES_HOST")
        .context("Unable to read POSTGRES_HOST environment variable")?;
    let db_name =
        std::env::var("POSTGRES_DB").context("Unable to read POSTGRES_DB environment variable")?;
    let db_username = std::env::var("POSTGRES_USERNAME")
        .context("Unable to read POSTGRES_USERNAME environment variable")?;
    let db_password = std::env::var("POSTGRES_PASSWORD")
        .context("Unable to read POSTGRES_PASSWORD environment variable")?;

    let db_connection = Database::connect(format!(
        "postgresql://{}:{}@{}/{}",
        db_username, db_password, db_host, db_name
    ))
    .await?;
    Migrator::up(&db_connection, None).await?;

    let server_state = Arc::new(ServerState {
        bot,
        channel_id,
        mailgun_webhook_signing_key,
        error_chat_id,
        allowed_senders,
        db_connection,
        update_token,
        crontap_client,
        crontap_client_id,
        crontap_api_key,
        webhook_update_url,
    });

    let router = Router::new()
        .route("/health", get(health))
        .route("/mail", post(receive_newsletter_email))
        .route("/update", post(update_latest_newsletter_message))
        .with_state(server_state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .context("Unable to bind listener to port 3000")?;
    axum::serve(listener, router)
        .await
        .context("Unable to start server")
}

async fn health() -> &'static str {
    "OK"
}

struct ServerState {
    bot: Bot,
    channel_id: ChatId,
    mailgun_webhook_signing_key: String,
    error_chat_id: ChatId,
    allowed_senders: HashSet<String>,
    db_connection: DatabaseConnection,
    update_token: String,
    crontap_client: Client,
    crontap_client_id: String,
    crontap_api_key: String,
    webhook_update_url: Url,
}

struct ServerError(anyhow::Error);

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for ServerError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct MailgunWebhookBody {
    from: String,
    #[serde(rename = "body-html")]
    html_body: String,
    token: String,
    signature: String,
    timestamp: u64,
}

fn verify_mailgun_signature(
    signing_key: &str,
    token: &str,
    timestamp: u64,
    signature: &str,
) -> anyhow::Result<()> {
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(format!("{}{}", timestamp, token).as_bytes());

    mac.verify_slice(&hex::decode(signature.as_bytes())?)
        .context("Unable to verify signature")
}

async fn receive_newsletter_email(
    State(state): State<Arc<ServerState>>,
    payload: Form<MailgunWebhookBody>,
) -> Result<(), ServerError> {
    async fn handle_email(
        state: Arc<ServerState>,
        Form(payload): Form<MailgunWebhookBody>,
    ) -> Result<(), ServerError> {
        info!("Received webhook from Mailgun");
        verify_mailgun_signature(
            &state.mailgun_webhook_signing_key,
            &payload.token,
            payload.timestamp,
            &payload.signature,
        )
        .context("Payload signature verification failed")?;

        if !state
            .allowed_senders
            .iter()
            .any(|s| payload.from.contains(s))
        {
            return Err(ServerError(anyhow!(
                "Got mail from unknown sender: {}",
                &payload.from
            )));
        }

        let newsletter_entry =
            parse_email_body(payload.html_body).context("Could not parse email body")?;

        let mut saved_newsletter =
            persist_newsletter_entry(&newsletter_entry, &state.db_connection)
                .await
                .context("Unable to persist newsletter entry")?;

        let message_text = make_message(&newsletter_entry);
        let message = state
            .bot
            .send_message(Recipient::Id(state.channel_id), message_text)
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .context("Unable to send update message")?;

        saved_newsletter.message_id = ActiveValue::Set(Some(message.id.0));
        saved_newsletter
            .save(&state.db_connection)
            .await
            .context("Unable to update newsletter with message id")?;

        update_schedules(state, newsletter_entry)
            .await
            .context("Unable to update schedules")?;

        Ok(())
    }

    if let Err(e) = handle_email(state.clone(), payload).await {
        error!("{:#}", e.0);

        let bot = &state.bot;
        bot.send_message(
            state.error_chat_id,
            format!("Got error while handling email: {:#}", e.0),
        )
        .await
        .context("Unable to send error message")?;
    }

    Ok(())
}

async fn update_latest_newsletter_message(
    State(state): State<Arc<ServerState>>,
    AuthBearer(token): AuthBearer,
) -> Result<(), ServerError> {
    async fn do_update(state: Arc<ServerState>, token: String) -> anyhow::Result<()> {
        if token != state.update_token {
            bail!("Invalid token");
        }

        let (newsletter, message_id) = fetch_latest_newsletter(&state.db_connection)
            .await
            .context("Unable to get latest newsletter from db")?;

        let updated_text = make_message(&newsletter);

        let mut joinset: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let _state = state.clone();
        joinset.spawn(async move {
            let state = _state;
            let updated_text = updated_text;
            state
                .bot
                .edit_message_text(state.channel_id, message_id, updated_text)
                .parse_mode(ParseMode::MarkdownV2)
                .await
                .context("Unable to update message")?;

            Ok(())
        });

        let _state = state.clone();
        joinset.spawn(async move {
            let state = _state;
            let newsletter = newsletter;
            update_schedules(state.clone(), newsletter)
                .await
                .context("Unable to update schedules")?;

            Ok(())
        });

        let results = joinset.join_all().await;

        if !results.is_empty() {
            let error_string = results
                .into_iter()
                .filter_map(|r| r.err())
                .map(|e| format!("{:?}", e))
                .join("\n");

            bail!("{}", error_string);
        }

        Ok(())
    }

    if let Err(e) = do_update(state.clone(), token).await {
        error!("{:#}", e);

        state
            .bot
            .send_message(
                state.error_chat_id,
                format!("Got error while updating newsletter message: {:#}", e),
            )
            .await
            .context("Unable to send error message")?;
    }

    Ok(())
}

async fn fetch_latest_newsletter(
    db_connection: &DatabaseConnection,
) -> anyhow::Result<(NewsletterEntry, MessageId)> {
    let latest_newsletter = entity::newsletter::Entity::find()
        .order_by_desc(entity::newsletter::Column::CreatedAt)
        .one(db_connection)
        .await
        .context("Could not fetch latest newsletter from db")?
        .ok_or(anyhow!("No newsletters in db"))?;

    let newsletter_programs = latest_newsletter
        .find_related(entity::program::Entity)
        .all(db_connection)
        .await
        .context("Could not fetch newsletter programs")?;

    let program_entries = newsletter_programs
        .load_many(entity::entry::Entity, db_connection)
        .await
        .context("Could not fetch program entries")?;

    let programming_entries: Vec<_> = newsletter_programs
        .into_iter()
        .zip(program_entries)
        .map(|(program, entries)| ProgrammingEntry {
            title: program.title,
            date_entries: entries
                .into_iter()
                .map(|e| DateEntry {
                    date: e.date.with_timezone(&Europe::Rome),
                    additional_details: e.details,
                })
                .collect(),
        })
        .collect();

    let newsletter = NewsletterEntry {
        programming_entries,
        newsletter_link: latest_newsletter.link,
    };

    Ok((
        newsletter,
        MessageId(
            latest_newsletter
                .message_id
                .ok_or(anyhow!("Message id for newsletter is not set"))?,
        ),
    ))
}

async fn update_schedules(
    state: Arc<ServerState>,
    newsletter_entry: NewsletterEntry,
) -> anyhow::Result<()> {
    const BOT_SCHEDULE_LABEL: &str = "bot_schedule";
    let schedules = state
        .crontap_client
        .list_schedules(
            None,
            None,
            None,
            Some(&state.crontap_api_key),
            Some(&state.crontap_client_id),
        )
        .await
        .context("Unable to list schedules from crontap")?
        .into_inner()
        .schedules;

    let mut bot_schedules = schedules
        .into_iter()
        .filter(|s| s.label == BOT_SCHEDULE_LABEL)
        .collect::<Vec<_>>();

    ensure!(!bot_schedules.is_empty(), "Unable to find any bot schedule");

    let last_schedule = bot_schedules.pop();
    for (index, schedule) in bot_schedules.into_iter().enumerate() {
        let state = state.clone();
        tokio::task::spawn(async move {
            tokio::time::sleep(Duration::from_secs((index + 1) as u64)).await;
            let result = state
                .crontap_client
                .delete_schedule_by_id(
                    &schedule.id,
                    Some(&state.crontap_client_id),
                    Some(&state.crontap_api_key),
                )
                .await
                .context("Unable to delete bot schedule");

            if let Err(e) = result {
                let error_result = state
                    .bot
                    .send_message(
                        state.error_chat_id,
                        format!("Got error while deleting bot schedule: {:#}", e),
                    )
                    .await
                    .context("Unable to send error message");

                if let Err(e) = error_result {
                    error!("{:#}", e);
                }
            }

            Ok::<(), anyhow::Error>(())
        });
    }

    let webhook_update_url = state.webhook_update_url.clone();
    let next_update_time = newsletter_entry
        .programming_entries
        .into_iter()
        .flat_map(|p| p.date_entries)
        .map(|d| d.date)
        .sorted()
        .find(|d| d >= &Utc::now());

    if let Some(next_update_time) = next_update_time {
        let headers = {
            let mut m = HashMap::new();
            m.insert(
                "Authorization".to_string(),
                format!("Bearer {}", state.update_token),
            );
            m
        };
        let added_schedule: AddSchedule = AddSchedule {
            data: None,
            headers: Some(KeyValue(headers)),
            integrations: None,
            interval: format!(
                "{} {} {} {} *",
                next_update_time.minute(),
                next_update_time.hour(),
                next_update_time.day(),
                next_update_time.month()
            ),
            label: BOT_SCHEDULE_LABEL.to_string(),
            timezone: Timezone("Europe/Rome".to_string()),
            url: webhook_update_url.to_string(),
            verb: "POST".to_string(),
        };

        if let Some(schedule) = last_schedule {
            state
                .crontap_client
                .update_schedule_by_id(
                    &schedule.id,
                    Some(&state.crontap_api_key),
                    Some(&state.crontap_client_id),
                    &added_schedule,
                )
                .await
                .context("Error while updating schedule")?;
        } else {
            state
                .crontap_client
                .create_schedule(
                    Some(&state.crontap_api_key),
                    Some(&state.crontap_client_id),
                    &added_schedule,
                )
                .await
                .context("Error while creating schedule")?;
        }
    }

    Ok(())
}

fn make_message(newsletter_entry: &NewsletterEntry) -> String {
    let entries_text = newsletter_entry
        .programming_entries
        .iter()
        .map(format_programming_entry)
        .join("\n\n");
    format!(
        "\
_Nuovi film in arrivo allo Spazio Alfieri\\!_

{}

[👉 Apri nel browser 🔗]({})
    ",
        entries_text, newsletter_entry.newsletter_link
    )
}

fn format_programming_entry(entry: &ProgrammingEntry) -> String {
    let mut formats_with_dates = entry
        .date_entries
        .iter()
        .map(|date_entry| {
            let human_readable_date = date_entry.date.format("%d/%m/%Y");
            let human_readable_time = date_entry.date.format("%H:%M");

            let strikethrough = {
                if Utc::now() > date_entry.date {
                    "~"
                } else {
                    ""
                }
            };

            // todo: make clock emoji represent time
            let formatted = format!(
                " {}• 📆 {} 🕔 {}{}{}",
                strikethrough,
                human_readable_date,
                human_readable_time,
                date_entry
                    .additional_details
                    .as_ref()
                    .map(|info| format!(" _{}_", markdown::escape(info)))
                    .as_deref()
                    .unwrap_or(""),
                strikethrough,
            );

            (formatted, date_entry.date)
        })
        .collect::<Vec<_>>();

    let nearest_date = formats_with_dates
        .iter_mut()
        .find(|(_, date)| Utc::now() <= *date);

    if let Some((formatted, _)) = nearest_date {
        *formatted = format!("{} 🔔", formatted)
    }

    let formatted_dates = formats_with_dates
        .into_iter()
        .map(|(formatted, _)| formatted)
        .join("\n");

    format!(
        "\
*{}*
Prossime date:
{}
    ",
        markdown::escape(&entry.title),
        formatted_dates
    )
}

async fn persist_newsletter_entry(
    newsletter_entry: &NewsletterEntry,
    connection: &DatabaseConnection,
) -> anyhow::Result<entity::newsletter::ActiveModel> {
    let newsletter = {
        let newsletter = entity::newsletter::ActiveModel {
            id: Default::default(),
            link: ActiveValue::Set(newsletter_entry.newsletter_link.clone()),
            message_id: Default::default(),
            created_at: Default::default(),
        };

        newsletter
            .save(connection)
            .await
            .context("Unable to save newsletter")?
    };

    let (programs, program_entries) = {
        let (programs, program_entries): (Vec<_>, Vec<_>) = newsletter_entry
            .programming_entries
            .iter()
            .map(|e| {
                let program = entity::program::ActiveModel {
                    id: ActiveValue::NotSet,
                    newsletter_id: newsletter.id.clone(),
                    title: ActiveValue::Set(e.title.clone()),
                };

                let date_entries: Vec<_> = e
                    .date_entries
                    .iter()
                    .map(|e| entity::entry::ActiveModel {
                        id: Default::default(),
                        program_id: Default::default(),
                        date: ActiveValue::Set(e.date.fixed_offset()),
                        details: ActiveValue::Set(e.additional_details.clone()),
                    })
                    .collect();

                (program, date_entries)
            })
            .collect();

        let mut saved_programs = Vec::new();
        for p in programs {
            saved_programs.push(
                p.save(connection)
                    .await
                    .context("Unable to save program!")?,
            );
        }

        (saved_programs, program_entries)
    };

    let entries_iter = program_entries
        .into_iter()
        .zip(programs.into_iter())
        .flat_map(|(es, p)| {
            es.into_iter().map(move |mut e| {
                e.program_id = p.id.clone();
                e
            })
        });

    entity::entry::Entity::insert_many(entries_iter)
        .exec(connection)
        .await
        .context("Unable to save entries")?;

    Ok(newsletter)
}
