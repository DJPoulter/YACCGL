# Linux build/test environment for developing on a non-Linux host.
FROM rust:1-trixie
RUN apt-get update && apt-get install -y --no-install-recommends \
      libgtk-4-dev libadwaita-1-dev pkg-config \
      xvfb xauth dbus-x11 imagemagick p7zip-full xdotool \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy rustfmt
WORKDIR /src
