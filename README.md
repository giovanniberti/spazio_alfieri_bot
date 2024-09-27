# Spazio Alfieri Bot

This is a Telegram bot that parses emails of the [Spazio Alfieri](spazioalfieri.it) cinema newsletter
and forwards its content to a given channel.

Written with [Rust](https://rust-lang.org) using the [Teloxide](https://docs.rs/teloxide) library.

The bot uses [MailGun](https://www.mailgun.com/) under the hood to receive email bodies.

## Building and running

To build the bot, [install the Rust toolchain](https://www.rust-lang.org/tools/install) and run

```shell
$ cargo build --release
```

Then run the resulting binary with
```shell
$ cargo run
```

or directly from the `target/release` directory.

### Setting up the environment

The bot expects the following environment variables, either from the OS environment
or from a `.env` file placed in the same directory as the executable:

| Variable        | Description                                                                 |
|-----------------|-----------------------------------------------------------------------------|
| MAILGUN_API_KEY | [MailGun](https://www.mailgun.com/) API key                                 |
| TELOXIDE_TOKEN  | [Telegram bot token](https://core.telegram.org/bots/#how-do-i-create-a-bot) |
| CHANNEL_ID      | Channel id where messages will be pusblished to                             |
| ERROR_CHAT_ID   | Chat id for reporting error messages                                        |
| ALLOWED_SENDERS | Comma-separated list of allowed email senders (email addresses)             |

All environment variables are required.