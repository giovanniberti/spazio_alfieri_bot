#![feature(iter_array_chunks)]

mod parser;

use crate::parser::parse_email_body;
use anyhow::Context;
use axum::extract::State;
use axum::routing::post;
use axum::{Form, Router};
use hmac::digest::MacError;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::str::FromStr;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::Recipient;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    info!("Starting SpazioAlfieriBot...");

    let bot = Bot::from_env();
    let channel_id = {
        let raw =
            std::env::var("CHANNEL_ID").context("Unable to get environment variable CHANNEL_ID")?;

        ChatId(i64::from_str(&raw).context("Unable to parse channel id into i64")?)
    };

    let mailgun_api_key = std::env::var("MAILGUN_API_KEY")
        .context("Unable to get environment variable MAILGUN_API_KEY")?;

    let server_state = Arc::new(ServerState {
        bot,
        channel_id,
        mailgun_api_key,
    });

    let router = Router::new()
        .route("/message", post(post_message))
        .route("/mail", post(receive_newsletter_email))
        .with_state(server_state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .context("Unable to bind listener to port 3000")?;
    axum::serve(listener, router)
        .await
        .context("Unable to start server")
}

struct ServerState {
    bot: Bot,
    channel_id: ChatId,
    mailgun_api_key: String,
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
    api_key: &str,
    token: &str,
    timestamp: u64,
    signature: &str,
) -> Result<(), MacError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(api_key.as_bytes()).expect("HMAC can take key of any size");
    mac.update(format!("{}{}", timestamp, token).as_bytes());

    mac.verify_slice(signature.as_bytes())
}

async fn receive_newsletter_email(
    State(state): State<Arc<ServerState>>,
    Form(payload): Form<MailgunWebhookBody>,
) -> Result<(), String> {
    info!("Received webhook from Mailgun");
    verify_mailgun_signature(
        &state.mailgun_api_key,
        &payload.token,
        payload.timestamp,
        &payload.signature,
    )
    .map_err(|e| format!("Payload signature verification failed: {}", e))?;

    let entries = parse_email_body(payload.html_body)
        .map_err(|e| format!("Could not parse email body: {}", e))?;
    println!("Got entries: {:?}", entries);

    Ok(())
}

async fn post_message(
    State(state): State<Arc<ServerState>>,
    message: String,
) -> Result<(), String> {
    let bot = &state.bot;

    info!("Sending message...");
    bot.send_message(Recipient::Id(state.channel_id), message)
        .await
        .map_err(|e| format!("Unable to send message: {}", e))?;

    Ok(())
}
