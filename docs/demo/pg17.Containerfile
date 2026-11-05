# The image the Postgres movies record in: rust:1.97-trixie (as CI) with
# PGDG Postgres 17, cargo-pgrx and python3-pexpect. docs/demo/pg-container.sh
# builds it (podman caches the layers) and runs a demo script in it.
FROM docker.io/library/rust:1.97-trixie
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends postgresql-common ca-certificates curl gnupg >/dev/null \
 && /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y >/dev/null \
 && apt-get install -y -qq --no-install-recommends postgresql-17 postgresql-server-dev-17 libclang-dev clang sudo python3 python3-pexpect git >/dev/null \
 && apt-get clean
RUN cargo install cargo-pgrx --version 0.19.3 --locked
# The demo runs as the calling user (podman --userns=keep-id): let it install
# the extension and use cargo's home.
RUN chmod -R a+w /usr/share/postgresql/17/extension /usr/lib/postgresql/17/lib && chmod -R a+rwX /usr/local/cargo
