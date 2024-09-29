#![feature(iter_array_chunks)]

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use hmac::{Hmac, Mac};
use itertools::Itertools;
use migration::{Migrator, MigratorTrait};
use sea_orm::Database;
use serde::Deserialize;
use sha2::Sha256;
use teloxide::prelude::*;
use teloxide::types::{ParseMode, Recipient};
use tracing::level_filters::LevelFilter;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::parser::{parse_email_body, NewsletterEntry, ProgrammingEntry};

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

    let db_host = std::env::var("POSTGRES_HOST")
        .context("Unable to read POSTGRS_HOST environment variable")?;
    let db_name =
        std::env::var("POSTGRES_DB").context("Unable to read POSTGRES_DB environment variable")?;
    let db_username = std::env::var("POSTGRES_USERNAME")
        .context("Unable to read POSTGRES_USERNAME environment variable")?;
    let db_password = std::env::var("POSTGRES_PASSWORD")
        .context("Unable to read POSTGRES_PASSWORD environment variable")?;

    let connection = Database::connect(format!(
        "postgresql://{}:{}@{}/{}",
        db_username, db_password, db_host, db_name
    ))
    .await?;
    Migrator::up(&connection, None).await?;

    let server_state = Arc::new(ServerState {
        bot,
        channel_id,
        mailgun_webhook_signing_key,
        error_chat_id,
        allowed_senders,
    });

    let router = Router::new()
        .route("/health", get(health))
        .route("/mail", post(receive_newsletter_email))
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
        let message = make_message(&newsletter_entry);

        state
            .bot
            .send_message(Recipient::Id(state.channel_id), message)
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .context("Unable to send update message")?;

        Ok(())
    }

    if let Err(e) = handle_email(state.clone(), payload).await {
        error!("{:#}", e.0);

        let bot = &state.bot;
        bot.send_message(
            Recipient::Id(state.error_chat_id),
            format!("Got error while handling email: {:#}", e.0),
        )
        .await
        .context("Unable to send error message")?;
    }

    Ok(())
}

fn make_message(newsletter_entry: &NewsletterEntry) -> String {
    let entries_text = newsletter_entry
        .programming_entries
        .iter()
        .map(format_message)
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

fn format_message(entry: &ProgrammingEntry) -> String {
    let formatted_dates = entry
        .date_entries
        .iter()
        .map(|date_entry| {
            let human_readable_date = date_entry.date.format("%d/%m/%Y");
            let human_readable_time = date_entry.date.format("%H:%M");

            // todo: make clock emoji represent time
            format!(
                " • 📆 {} 🕔 {} {}",
                human_readable_date,
                human_readable_time,
                date_entry
                    .additional_details
                    .as_ref()
                    .map(|info| format!("_{}_", info))
                    .as_deref()
                    .unwrap_or("")
            )
        })
        .join("\n");

    format!(
        "\
*{}*
Prossime date:
{}
    ",
        entry.title, formatted_dates
    )
}
