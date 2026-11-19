# Brainkeep server image: Railway, Docker or any container host.
# Data and generated credentials live in /data (mount a volume there).
# See docs/railway.md for the variables.
FROM rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bin brainkeep

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends tini \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10001 brainkeep \
 && useradd --system --uid 10001 --gid 10001 --home-dir /data --shell /usr/sbin/nologin brainkeep
COPY --from=build /src/target/release/brainkeep /usr/local/bin/brainkeep
COPY --chmod=0755 deploy/docker-entrypoint.sh /usr/local/bin/brainkeep-entrypoint
ENV BRAINKEEP_ROOT=/data PORT=18765
# No VOLUME instruction: Railway rejects it. Mount a volume at /data (docker run -v ...:/data).
EXPOSE 18765
# Starts as root only to take ownership of a root-owned volume, then drops to brainkeep.
ENTRYPOINT ["tini", "--", "/usr/local/bin/brainkeep-entrypoint"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s \
  CMD ["setpriv", "--reuid=10001", "--regid=10001", "--clear-groups", "--no-new-privs", "brainkeep", "--data-dir", "/data/store", "status"]
