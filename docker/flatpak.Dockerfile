# Builds the Flatpak bundle. Needs `docker run --privileged` for bubblewrap.
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      flatpak flatpak-builder ca-certificates git appstream librsvg2-common \
    && rm -rf /var/lib/apt/lists/* \
    && flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
WORKDIR /src
