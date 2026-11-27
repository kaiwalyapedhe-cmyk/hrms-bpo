# syntax=docker/dockerfile:1.7
# ---------------------------------------------------------------------------
# Leptos CSR client compiled to WASM by Trunk, served as static files.
# Build context: repository root.
# ---------------------------------------------------------------------------
FROM ghcr.io/trunk-rs/trunk:0.21.14 AS build
WORKDIR /app
COPY . .
RUN cd crates/web && trunk build --release --dist /dist

FROM nginx:1.27-alpine AS runtime
COPY docker/web-static.conf /etc/nginx/conf.d/default.conf
COPY --from=build /dist /usr/share/nginx/html
EXPOSE 80
HEALTHCHECK --interval=15s --timeout=3s --retries=5 \
  CMD wget -q -O /dev/null http://127.0.0.1/ || exit 1
