```dockerfile
# syntax=docker/dockerfile:1
FROM rust:1.80-slim AS build
ARG VERSION=1.0
ENV CARGO_HOME=/cargo \
    PATH="/cargo/bin:$PATH"
WORKDIR /src
COPY --from=deps /cargo /cargo
COPY . .
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN cargo build --release

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/x/y"
EXPOSE 8080/tcp
USER 1000:1000
HEALTHCHECK CMD curl -f http://localhost:8080/ || exit 1
ENTRYPOINT ["/usr/local/bin/app"]
CMD ["--port", "8080"]
```
