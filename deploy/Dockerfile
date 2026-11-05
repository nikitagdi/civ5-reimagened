# Build context: target/dist/civ5-<build>/ (cargo xtask dist): civ5-server, web/, server.toml, this file.
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends libc++1-18 libc++abi1-18 \
 && rm -rf /var/lib/apt/lists/* \
 && install -d -o 1000 -g 1000 /var/lib/civ5 /srv/civ5/assets /srv/civ5/gamedata /etc/civ5
COPY civ5-server /usr/local/bin/civ5-server
COPY web /srv/civ5/web
COPY server.toml /etc/civ5/
# uid 1000 (user `ubuntu` of the base image) owns /srv/civ5 on the host; the worker never runs as root.
USER 1000:1000
EXPOSE 8080
ENTRYPOINT ["civ5-server"]
CMD ["--config", "/etc/civ5/server.toml"]
