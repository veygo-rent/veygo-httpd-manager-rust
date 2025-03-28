FROM rust:slim

WORKDIR /app

COPY . ./

RUN apt update && apt install curl pkg-config git libssl-dev libsodium-dev libpq-dev -y

RUN cargo install diesel_cli --no-default-features --features postgres

RUN cargo build --release

ENTRYPOINT ./target/release/veygo-task-manager-rust

EXPOSE 8000

