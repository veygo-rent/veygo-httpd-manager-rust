FROM rust:slim

WORKDIR /app

COPY . ./

RUN curl --proto '=https' --tlsv1.2 -LsSf https://github.com/diesel-rs/diesel/releases/latest/download/diesel_cli-installer.sh | sh

RUN cargo build --release

ENTRYPOINT ./target/release/veygo-task-manager-rust

EXPOSE 8000

