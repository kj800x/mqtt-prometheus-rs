FROM rust:1.96

WORKDIR /usr/src/prom-money-rs
COPY . .

RUN cargo install --path .

CMD ["prom-money-rs"]
