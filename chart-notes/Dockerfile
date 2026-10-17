# syntax=docker/dockerfile:1
# Built from the repository root: docker build -f chart-notes/Dockerfile .
FROM rust:1.94-bookworm@sha256:6ae102bdbf528294bc79ad6e1fae682f6f7c2a6e6621506ba959f9685b308a55 AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --package chart-notes --bin chart-notes \
 && cp target/release/chart-notes /chart-notes

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
COPY --from=build /chart-notes /usr/local/bin/chart-notes
USER 65532:65532
EXPOSE 8080 9090
ENTRYPOINT ["/usr/local/bin/chart-notes"]
