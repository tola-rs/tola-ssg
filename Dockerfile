# syntax=docker/dockerfile:1

ARG ALPINE=3.24

FROM alpine:${ALPINE} AS fetch
ARG VERSION
ARG TARGETARCH
RUN apk add --no-cache ca-certificates curl
RUN test -n "${VERSION}" || { echo "VERSION build-arg is required" >&2; exit 1; }
RUN case "${TARGETARCH}" in \
      amd64) arch=x86_64 ;; \
      arm64) arch=aarch64 ;; \
      *) echo "unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac && \
    base="https://github.com/tola-rs/tola-ssg/releases/download/v${VERSION}" && \
    name="tola-${VERSION}-${arch}-unknown-linux-musl.tar.gz" && \
    curl -fsSLo "/tmp/${name}" "${base}/${name}" && \
    curl -fsSLo /tmp/checksums.txt "${base}/checksums.txt" && \
    expected="$(awk -v n="${name}" '$2 == n { print $1 }' /tmp/checksums.txt)" && \
    actual="$(sha256sum "/tmp/${name}" | cut -d' ' -f1)" && \
    test -n "${expected}" && test "${expected}" = "${actual}" && \
    tar -xzf "/tmp/${name}" -C /usr/local/bin

FROM alpine:${ALPINE}
RUN apk add --no-cache ca-certificates tzdata
COPY --from=fetch /usr/local/bin/tola /usr/local/bin/tola
WORKDIR /app
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/tola"]
