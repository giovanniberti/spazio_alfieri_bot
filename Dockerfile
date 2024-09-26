FROM rustlang/rust:nightly AS chef
RUN addgroup --system user && adduser --ingroup user --system user && mkdir /app && chown user:user /app
USER user:user
WORKDIR /app
RUN cargo +nightly install cargo-chef

FROM chef AS planner
USER user:user
COPY --chown=user . .
RUN cargo +nightly chef prepare --recipe-path ./recipe.json

FROM chef AS cacher
USER user:user
WORKDIR app
COPY --chown=user --from=planner /app/recipe.json recipe.json
RUN cargo +nightly chef cook --release --recipe-path ./recipe.json

FROM rustlang/rust:nightly-slim AS builder
RUN apt update -y && apt install pkg-config libssl-dev -y
RUN addgroup --system user && adduser --ingroup user --system user && mkdir /app && chown user:user /app
USER user:user
WORKDIR app
COPY --chown=user . .
RUN cargo +nightly build --release --bin spazio_alfieri_bot

FROM rustlang/rust:nightly-slim AS runtime
COPY --from=builder /app/target/release/spazio_alfieri_bot /usr/local/bin
ENTRYPOINT ["/usr/local/bin/spazio_alfieri_bot"]
