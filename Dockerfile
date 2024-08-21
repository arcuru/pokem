FROM rust:bookworm AS builder
WORKDIR /usr/src/pokem
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y openssl libsqlite3-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/pokem/target/release/pokem /usr/local/bin/pokem
CMD ["pokem", "--daemon", "--config", "/config.yaml"]
