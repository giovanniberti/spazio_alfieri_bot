use std::str::FromStr;
use std::sync::Arc;
use anyhow::Context;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use teloxide::prelude::*;
use teloxide::types::Recipient;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    info!("Starting SpazioAlfieriBot...");

    let bot = Bot::from_env();
    let channel_id = {
        let raw = std::env::var("CHANNEL_ID")
            .context("Unable to get environment variable CHANNEL_ID")?;

        ChatId(i64::from_str(&raw).context("Unable to parse channel id into i64")?)
    };

    let server_state = Arc::new(ServerState { bot, channel_id });

    let router = Router::new()
        .route("/message", post(post_message))
        .with_state(server_state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.context("Unable to bind listener to port 3000")?;
    axum::serve(listener, router).await.context("Unable to start server")
}

struct ServerState {
    bot: Bot,
    channel_id: ChatId
}

async fn post_message(
    State(state): State<Arc<ServerState>>,
    message: String,
) -> Result<(), String> {
    let bot = &state.bot;

    info!("Sending message...");
    bot.send_message(Recipient::Id(state.channel_id), message).await.map_err(|e| format!("Unable to send message: {}", e))?;

    Ok(())
}
