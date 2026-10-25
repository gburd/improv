# improv-server (and the improv CLI) in a container: ghcr.io/gburd/improv.
#
#   podman build -t improv -f Containerfile .
#   podman run --rm -p 3000:3000 -v "$PWD:/data" \
#     -e IMPROV_API_TOKEN_FILE=/run/secrets/improv-tokens improv /data/sales.db
#
# The server binds IMPROV_ADDR=0.0.0.0:3000 inside the container (an address
# argument overrides it), so it refuses to start
# without a token (IMPROV_API_TOKEN, IMPROV_API_TOKENS or
# IMPROV_API_TOKEN_FILE) unless IMPROV_ALLOW_OPEN=1. It runs as an
# unprivileged user; the model directory must be writable by uid 10001.
FROM docker.io/library/rust:1.97-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p improv_server -p improv_cli \
 && strip target/release/improv-server target/release/improv

FROM docker.io/library/debian:trixie-slim
RUN useradd --system --uid 10001 --home-dir /data improv \
 && mkdir -p /data && chown improv /data
COPY --from=build /src/target/release/improv-server /src/target/release/improv /usr/local/bin/
COPY LICENSE-APACHE LICENSE-MIT /usr/share/doc/improv/
USER improv
WORKDIR /data
ENV IMPROV_ADDR=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["improv-server"]
CMD ["/data/improv.db"]
