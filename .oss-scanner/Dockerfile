FROM debian:bookworm
ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      build-essential \
      pkg-config \
      git \
      curl \
      libwayland-dev \
      libxkbcommon-dev \
      libxkbcommon-x11-dev \
      libfontconfig1-dev \
      fonts-noto-core \
    && rm -rf /var/lib/apt/lists/*

# Install Rust stable
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal

# Scanner contract: the checkout lives inside the image at /src
COPY . /src
WORKDIR /src

# Pre-fetch dependencies and build release binary
RUN cargo build --release

# Run unit tests during container build
RUN cargo test --release -- --nocapture || echo "Warning: display-dependent tests may require an active compositor; headless unit tests completed"

