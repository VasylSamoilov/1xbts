# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
        protobuf-compiler libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=1xbts-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=1xbts-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=1xbts-femto-target,target=/src/target,sharing=locked \
    cargo build --release -p cdma-femto --bins \
    && cp /src/target/release/cdma-femto /tmp/cdma-femto \
    && cp /src/target/release/cdma-femto-segw-config /tmp/cdma-femto-segw-config

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /tmp/cdma-femto /usr/local/bin/cdma-femto
COPY --from=build /tmp/cdma-femto-segw-config /usr/local/bin/cdma-femto-segw-config
ENTRYPOINT ["/usr/local/bin/cdma-femto"]
